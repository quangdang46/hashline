//! Self-update: query GitHub Releases, compare versions, download the
//! platform asset with checksum verification, and replace the running
//! binary. Also powers the periodic "update available" notice.
//!
//! All endpoints can be redirected to a mirror base URL with
//! `HASHLINE_RELEASES_BASE_URL` (used by tests and self-hosted mirrors):
//! `{base}/releases/latest` serves the release JSON, `{base}/download/...`
//! serves release assets, and `{base}/latest` redirects to the latest tag.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;
use ureq::ResponseExt;

use crate::cli::Commands;

pub const GITHUB_OWNER: &str = "quangdang46";
pub const GITHUB_REPO: &str = "hashline";

/// How often the automatic update notice refreshes its cached view of the
/// latest release.
const NOTICE_REFRESH_INTERVAL: u64 = 24 * 60 * 60;

/// A GitHub release, as tag (`v0.9.16`) plus bare version (`0.9.16`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
}

impl Release {
    /// Build from a bare version, normalizing the `v`-prefixed tag.
    pub fn from_version(version: &str) -> Self {
        let version = version.trim().trim_start_matches('v').to_string();
        Self {
            tag: format!("v{version}"),
            version,
        }
    }
}

/// Version triple compared to decide whether an update is available.
///
/// Deriving `Ord` over `major, minor, patch` in field order gives exactly
/// the numeric comparison we need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Semver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

/// Parse `v?MAJOR.MINOR.PATCH(-pre)?(+build)?` into a comparable triple.
pub fn parse_version(text: &str) -> Option<Semver> {
    let trimmed = text.trim();
    let core = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Semver {
        major,
        minor,
        patch,
    })
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Outcome of comparing the installed version against a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Outdated { latest: String },
    Unknown,
}

pub fn compare_status(installed: &str, latest: &str) -> UpdateStatus {
    match (parse_version(installed), parse_version(latest)) {
        (Some(a), Some(b)) if b > a => UpdateStatus::Outdated {
            latest: latest.to_string(),
        },
        (Some(_), Some(_)) => UpdateStatus::UpToDate,
        _ => UpdateStatus::Unknown,
    }
}

fn releases_base_override() -> Option<String> {
    std::env::var("HASHLINE_RELEASES_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

fn api_latest_url() -> String {
    match releases_base_override() {
        Some(base) => format!("{base}/releases/latest"),
        None => {
            format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest")
        }
    }
}

fn latest_page_url() -> String {
    match releases_base_override() {
        Some(base) => format!("{base}/latest"),
        None => format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest"),
    }
}

fn asset_download_url(tag: &str, asset: &str) -> String {
    match releases_base_override() {
        Some(base) => format!("{base}/download/{tag}/{asset}"),
        None => {
            format!(
                "https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/download/{tag}/{asset}"
            )
        }
    }
}

fn user_agent() -> String {
    format!("hashline/{}", current_version())
}

fn http_agent(connect_timeout: Duration, global_timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(connect_timeout))
        .timeout_global(Some(global_timeout))
        .build()
        .new_agent()
}

/// Release asset platform key. Must stay in sync with the release workflow
/// matrix (`.github/workflows/release.yml`) and `install.sh`.
pub fn release_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        (os, arch) => Err(format!(
            "unsupported platform {os}-{arch}: no release asset is built for it"
        )),
    }
}

fn archive_ext(platform: &str) -> &'static str {
    if platform.starts_with("windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

#[derive(Deserialize)]
struct LatestReleaseJson {
    tag_name: String,
}

/// Query the latest release: GitHub API first, then the `releases/latest`
/// redirect as a fallback (the API is rate-limited for anonymous callers).
pub fn fetch_latest_release() -> Result<Release, String> {
    fetch_via_api().or_else(|api_error| {
        debug!(error = %api_error, "github api release lookup failed, trying redirect fallback");
        fetch_via_redirect()
    })
}

fn fetch_via_api() -> Result<Release, String> {
    let agent = http_agent(Duration::from_secs(5), Duration::from_secs(15));
    let mut response = agent
        .get(api_latest_url())
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("release lookup failed: {e}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("release lookup body read failed: {e}"))?;
    let parsed: LatestReleaseJson =
        serde_json::from_str(&body).map_err(|e| format!("release json parse failed: {e}"))?;
    Ok(Release::from_version(&parsed.tag_name))
}

fn fetch_via_redirect() -> Result<Release, String> {
    let agent = http_agent(Duration::from_secs(5), Duration::from_secs(15));
    let response = agent
        .get(latest_page_url())
        .header("User-Agent", user_agent())
        .call()
        .map_err(|e| format!("release redirect lookup failed: {e}"))?;
    let uri = response.get_uri().to_string();
    let tag = uri
        .split_once("/tag/")
        .map(|(_, rest)| rest.to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("could not find a release tag in redirect target '{uri}'"))?;
    Ok(Release::from_version(&tag))
}

/// Download `url` to `dest`. Returns `Ok(false)` when the server answers
/// 404 (e.g. a release that publishes no checksum asset).
fn download_file(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<bool, String> {
    let response = match agent.get(url).header("User-Agent", user_agent()).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(false),
        Err(e) => return Err(format!("download of {url} failed: {e}")),
    };
    let mut reader = response.into_body().into_reader();
    let mut file =
        fs::File::create(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    io::copy(&mut reader, &mut file)
        .map_err(|e| format!("download of {url} failed mid-stream: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("cannot flush {}: {e}", dest.display()))?;
    Ok(true)
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| format!("cannot hash {}: {e}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_checksum(archive: &Path, checksum_file: &Path) -> Result<(), String> {
    let raw = fs::read_to_string(checksum_file)
        .map_err(|e| format!("cannot read {}: {e}", checksum_file.display()))?;
    let expected = raw
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "checksum file {} does not contain a sha256 digest",
            checksum_file.display()
        ));
    }
    let actual = sha256_hex(archive)?;
    if actual != expected {
        return Err(format!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            archive.display()
        ));
    }
    Ok(())
}

/// Extract a release archive with the system `tar` (bsdtar on macOS and
/// Windows also reads zip, which is what the Windows release ships).
fn extract_archive(archive: &Path, into: &Path) -> Result<(), String> {
    let mut command = Command::new("tar");
    command.arg("-x");
    if archive.extension().is_some_and(|e| e == "gz") {
        command.arg("-z");
    }
    command.arg("-f").arg(archive).arg("-C").arg(into);
    let output = command
        .output()
        .map_err(|e| format!("cannot run tar to extract {}: {e}", archive.display()))?;
    if !output.status.success() {
        return Err(format!(
            "tar failed to extract {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn find_binary(dir: &Path) -> Result<PathBuf, String> {
    let wanted = if cfg!(windows) {
        "hashline.exe"
    } else {
        "hashline"
    };
    let mut stack = vec![dir.to_path_buf()];
    let mut dirs_scanned = 0usize;
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|e| format!("cannot list {}: {e}", current.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read entry in {}: {e}", current.display()))?;
            let path = entry.path();
            if path.is_dir() {
                dirs_scanned += 1;
                if dirs_scanned > 64 {
                    return Err("release archive layout is too deep to scan".into());
                }
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == wanted) {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "'{wanted}' not found in the extracted release archive"
    ))
}

/// Result of a successful self-update install.
#[derive(Clone, Debug)]
pub struct UpdateReport {
    pub previous: String,
    pub current: String,
    pub path: PathBuf,
    pub checksum_verified: bool,
}

/// Download `release`, verify its checksum, and replace the running binary.
pub fn download_and_install(release: &Release) -> Result<UpdateReport, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot locate the running binary: {e}"))?;
    download_and_install_into(release, &exe)
}

/// Like [`download_and_install`] but replaces `target` instead of the
/// running binary (used by tests).
pub fn download_and_install_into(release: &Release, target: &Path) -> Result<UpdateReport, String> {
    let platform = release_platform()?;
    let ext = archive_ext(platform);
    let asset = format!("hashline-{}-{platform}.{ext}", release.tag);
    let agent = http_agent(Duration::from_secs(10), Duration::from_secs(120));

    let scratch = tempfile::tempdir().map_err(|e| format!("cannot create temp dir: {e}"))?;
    let archive_path = scratch.path().join(&asset);
    let archive_url = asset_download_url(&release.tag, &asset);
    if !download_file(&agent, &archive_url, &archive_path)? {
        return Err(format!(
            "release asset '{asset}' not found for tag {}",
            release.tag
        ));
    }

    // The release workflow publishes `<asset>.sha256`; verify when present.
    // A missing checksum is not fatal — the download itself was TLS-authenticated.
    let checksum_verified = match download_file(
        &agent,
        &format!("{archive_url}.sha256"),
        &scratch.path().join("asset.sha256"),
    ) {
        Ok(true) => {
            verify_checksum(&archive_path, &scratch.path().join("asset.sha256"))?;
            true
        }
        Ok(false) => false,
        Err(e) => return Err(e),
    };

    let extract_dir = scratch.path().join("extracted");
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("cannot create {}: {e}", extract_dir.display()))?;
    extract_archive(&archive_path, &extract_dir)?;
    let new_binary = find_binary(&extract_dir)?;

    replace_binary(&new_binary, target)?;
    Ok(UpdateReport {
        previous: current_version().to_string(),
        current: release.version.clone(),
        path: target.to_path_buf(),
        checksum_verified,
    })
}

fn replace_binary(new_binary: &Path, target: &Path) -> Result<(), String> {
    let target_dir = target
        .parent()
        .ok_or_else(|| format!("invalid binary path {}", target.display()))?;
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("cannot create {}: {e}", target_dir.display()))?;
    let staged_name = format!(
        ".{}.update-{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("hashline"),
        std::process::id()
    );
    let staged = target_dir.join(staged_name);

    fs::copy(new_binary, &staged).map_err(|e| format!("cannot stage new binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot chmod staged binary: {e}"))?;
    }
    if let Ok(staged_file) = fs::File::open(&staged) {
        let _ = staged_file.sync_all();
    }

    // Windows keeps the running exe locked: move the old image aside first,
    // then put the new binary in place and best-effort delete the old one
    // (the delete succeeds on a later run once the process is gone).
    #[cfg(windows)]
    let retired = {
        let retired = target.with_extension("exe.old");
        let _ = fs::rename(target, &retired);
        retired
    };
    fs::rename(&staged, target).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
    #[cfg(windows)]
    {
        let _ = fs::remove_file(&retired);
    }
    Ok(())
}

// ── Periodic update notice ──────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct NoticeCache {
    last_check_epoch: u64,
    latest_version: String,
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

fn notice_cache_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".hashline").join("update-check.json"))
}

fn read_notice_cache(path: &Path) -> Option<NoticeCache> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_notice_cache(path: &Path, cache: &NoticeCache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = fs::write(path, json);
    }
}

/// Record the latest known release so the periodic notice stays consistent
/// after an explicit `hashline update` run.
pub fn record_release_seen(version: &str) {
    if let Some(path) = notice_cache_path() {
        write_notice_cache(
            &path,
            &NoticeCache {
                last_check_epoch: epoch_now(),
                latest_version: version.to_string(),
            },
        );
    }
}

/// Whether the automatic update notice may run for `command`: long-running
/// modes (serve/mcp) and the explicit `update` command are excluded, and
/// `HASHLINE_NO_UPDATE_CHECK` opts out entirely.
pub fn notice_eligible(command: &Commands) -> bool {
    if matches!(
        command,
        Commands::Serve(_) | Commands::Mcp(_) | Commands::Update(_)
    ) {
        return false;
    }
    std::env::var("HASHLINE_NO_UPDATE_CHECK").map_or(true, |v| v.trim().is_empty())
}

/// Testable core of the periodic notice: consult the cache, refresh it at
/// most once per [`NOTICE_REFRESH_INTERVAL`], and compare versions. The
/// `fetch` closure is only invoked when the cache is stale.
fn notice_status(
    cache_path: &Path,
    now: u64,
    fetch: impl FnOnce() -> Result<Release, String>,
) -> UpdateStatus {
    let cached = read_notice_cache(cache_path);
    if let Some(cache) = &cached {
        if now.saturating_sub(cache.last_check_epoch) < NOTICE_REFRESH_INTERVAL {
            return compare_status(current_version(), &cache.latest_version);
        }
    }
    match fetch() {
        Ok(release) => {
            write_notice_cache(
                cache_path,
                &NoticeCache {
                    last_check_epoch: now,
                    latest_version: release.version.clone(),
                },
            );
            compare_status(current_version(), &release.version)
        }
        Err(error) => {
            debug!(%error, "update notice refresh failed");
            // Back off until the next refresh window instead of hitting the
            // network (or DNS) on every single invocation while offline.
            write_notice_cache(
                cache_path,
                &NoticeCache {
                    last_check_epoch: now,
                    latest_version: cached
                        .as_ref()
                        .map(|c| c.latest_version.clone())
                        .unwrap_or_default(),
                },
            );
            match &cached {
                Some(cache) => compare_status(current_version(), &cache.latest_version),
                None => UpdateStatus::Unknown,
            }
        }
    }
}

/// Best-effort outdated-version notice printed to stderr after a successful
/// interactive command. Gated on a terminal stderr so agent pipelines keep
/// byte-identical output, and throttled to one network refresh per day.
pub fn maybe_print_update_notice(stderr_is_terminal: bool) {
    if !stderr_is_terminal || !notice_enabled() {
        return;
    }
    let Some(cache_path) = notice_cache_path() else {
        return;
    };
    let status = notice_status(&cache_path, epoch_now(), fetch_latest_release);
    if let UpdateStatus::Outdated { latest } = status {
        eprintln!(
            "NOTE update available: latest={latest} installed={} (run `hashline update`)",
            current_version()
        );
    }
}

fn notice_enabled() -> bool {
    std::env::var("HASHLINE_NO_UPDATE_CHECK").map_or(true, |v| v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_v_prefixed_versions() {
        assert_eq!(
            parse_version("0.9.15"),
            Some(Semver {
                major: 0,
                minor: 9,
                patch: 15
            })
        );
        assert_eq!(
            parse_version("v0.9.16"),
            Some(Semver {
                major: 0,
                minor: 9,
                patch: 16
            })
        );
        assert_eq!(
            parse_version("  V1.2.3  "),
            Some(Semver {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        // Pre-release and build suffixes are ignored for comparison.
        assert_eq!(
            parse_version("2.0.0-rc.1"),
            Some(Semver {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            parse_version("2.0.0+build.7"),
            Some(Semver {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
    }

    #[test]
    fn rejects_unparsable_versions() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("1.2.x"), None);
    }

    #[test]
    fn semver_ordering_is_numeric() {
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.99").unwrap());
        assert!(parse_version("1.0.0").unwrap() > parse_version("0.99.99").unwrap());
        assert!(parse_version("1.2.3").unwrap() == parse_version("v1.2.3").unwrap());
    }

    #[test]
    fn compare_status_classifies_versions() {
        assert_eq!(
            compare_status("0.9.15", "0.9.16"),
            UpdateStatus::Outdated {
                latest: "0.9.16".into()
            }
        );
        assert_eq!(compare_status("0.9.16", "v0.9.16"), UpdateStatus::UpToDate);
        assert_eq!(compare_status("0.9.17", "0.9.16"), UpdateStatus::UpToDate);
        assert_eq!(
            compare_status("0.9.15", "not-a-version"),
            UpdateStatus::Unknown
        );
    }

    #[test]
    fn release_from_version_normalizes_tag() {
        let release = Release::from_version("0.9.16");
        assert_eq!(release.tag, "v0.9.16");
        assert_eq!(release.version, "0.9.16");
        let prefixed = Release::from_version("v1.0.0");
        assert_eq!(prefixed.tag, "v1.0.0");
        assert_eq!(prefixed.version, "1.0.0");
    }

    #[test]
    fn release_platform_matches_host_triple() {
        let platform = release_platform().unwrap();
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => assert_eq!(platform, "linux-x86_64"),
            ("linux", "aarch64") => assert_eq!(platform, "linux-aarch64"),
            ("macos", "x86_64") => assert_eq!(platform, "macos-x86_64"),
            ("macos", "aarch64") => assert_eq!(platform, "macos-aarch64"),
            ("windows", "x86_64") => assert_eq!(platform, "windows-x86_64"),
            _ => unreachable!("CI only builds the platforms mapped in release_platform"),
        }
    }

    #[test]
    fn notice_status_uses_fresh_cache_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("update-check.json");
        write_notice_cache(
            &cache,
            &NoticeCache {
                last_check_epoch: epoch_now(),
                latest_version: "0.9.16".into(),
            },
        );
        let status = notice_status(&cache, epoch_now(), || -> Result<Release, String> {
            panic!("fetch must not run while the cache is fresh")
        });
        assert_eq!(
            status,
            UpdateStatus::Outdated {
                latest: "0.9.16".into()
            }
        );
    }

    #[test]
    fn notice_status_refreshes_stale_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("update-check.json");
        write_notice_cache(
            &cache,
            &NoticeCache {
                last_check_epoch: epoch_now() - NOTICE_REFRESH_INTERVAL - 1,
                latest_version: "0.9.15".into(),
            },
        );
        let status = notice_status(&cache, epoch_now(), || Ok(Release::from_version("0.9.20")));
        assert_eq!(
            status,
            UpdateStatus::Outdated {
                latest: "0.9.20".into()
            }
        );
        let refreshed = read_notice_cache(&cache).unwrap();
        assert_eq!(refreshed.latest_version, "0.9.20");
    }

    #[test]
    fn notice_status_backs_off_after_failed_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("update-check.json");
        let now = epoch_now();
        let mut calls = 0;
        let status = notice_status(&cache, now, || {
            calls += 1;
            Err("network unreachable".into())
        });
        assert_eq!(status, UpdateStatus::Unknown);
        // A second check inside the same window must not hit the network again.
        let status = notice_status(&cache, now + 60, || {
            calls += 1;
            Err("network unreachable".into())
        });
        assert_eq!(status, UpdateStatus::Unknown);
        assert_eq!(calls, 1);
    }

    #[test]
    fn notice_eligible_excludes_long_running_and_update_commands() {
        use crate::cli::{Commands, McpCmd, ReadCmd, ServeCmd, UpdateCmd};
        use std::path::PathBuf;

        assert!(!notice_eligible(&Commands::Serve(ServeCmd {
            socket: None,
            http: None,
            detach: false,
            pid_file: None,
        })));
        assert!(!notice_eligible(&Commands::Mcp(McpCmd {
            proxy_to_daemon: false,
        })));
        assert!(!notice_eligible(&Commands::Update(UpdateCmd {
            check: false,
            version: None,
            json: false,
        })));
        // Commands that are not excluded always consult the env toggle; only
        // assert the env-independent exclusion above for the true case.
        let _ = notice_eligible(&Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            json: false,
            no_cache: false,
        }));
        let _ = Duration::from_secs(0); // keep imports honest when cfg strips code
    }
}
