//! End-to-end tests for `hashline update` against a local release mirror.
//!
//! The mirror speaks the three endpoints the updater uses
//! (`/releases/latest` JSON, `/download/<tag>/<asset>` assets, and the
//! `/latest` → `/tag/<tag>` redirect fallback), so the full check /
//! download / verify / replace flow runs without touching GitHub.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use hashline::update::{self, Release};
use sha2::{Digest, Sha256};

// ── Release mirror ───────────────────────────────────────────────────

struct MirrorConfig {
    tag: String,
    fixture: Option<PathBuf>,
    /// Respond 500 on `/releases/latest` to exercise the redirect fallback.
    fail_api: bool,
}

struct Mirror {
    base: String,
}

fn spawn_mirror(cfg: MirrorConfig) -> Mirror {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mirror");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if serve_one(&mut stream, &cfg).is_err() {
                break;
            }
        }
    });
    Mirror {
        base: format!("http://127.0.0.1:{port}"),
    }
}

fn serve_one(stream: &mut TcpStream, cfg: &MirrorConfig) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte)? {
            1 => buf.push(byte[0]),
            _ => return Ok(()),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or("");
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let response = build_response(path, cfg);
    stream.write_all(&response)?;
    stream.flush()
}

fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "{status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn http_redirect(location: &str) -> Vec<u8> {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

fn build_response(path: &str, cfg: &MirrorConfig) -> Vec<u8> {
    if path == "/releases/latest" {
        if cfg.fail_api {
            return http_response("HTTP/1.1 500 Internal Server Error", "text/plain", b"boom");
        }
        let body = format!("{{\"tag_name\":\"{}\"}}", cfg.tag);
        return http_response("HTTP/1.1 200 OK", "application/json", body.as_bytes());
    }
    if path == "/latest" {
        return http_redirect(&format!("/tag/{}", cfg.tag));
    }
    if path == format!("/tag/{}", cfg.tag) {
        return http_response("HTTP/1.1 200 OK", "text/html", b"release page");
    }
    if let Some(fixture) = &cfg.fixture {
        let asset_name = fixture.file_name().unwrap().to_string_lossy().to_string();
        let asset_path = format!("/download/{}/{}", cfg.tag, asset_name);
        if path == asset_path {
            let bytes = std::fs::read(fixture).expect("read fixture archive");
            return http_response("HTTP/1.1 200 OK", "application/octet-stream", &bytes);
        }
        if path == format!("{asset_path}.sha256") {
            let bytes = std::fs::read(fixture).expect("read fixture archive");
            let hash = format!("{:x}", Sha256::digest(&bytes));
            return http_response("HTTP/1.1 200 OK", "text/plain", hash.as_bytes());
        }
    }
    http_response("HTTP/1.1 404 Not Found", "text/plain", b"not found")
}

// ── Fixtures ─────────────────────────────────────────────────────────

/// The tag the updater should offer: current package version with the
/// patch component bumped, so the test never depends on a hardcoded
/// release number.
fn next_version_tag() -> String {
    let mut components: Vec<u64> = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse().expect("numeric version component"))
        .collect();
    *components.last_mut().expect("semver has components") += 1;
    format!(
        "v{}",
        components
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".")
    )
}

fn current_version_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "hashline.exe"
    } else {
        "hashline"
    }
}

/// Compile a native executable on every platform, not a script masquerading
/// as a Windows executable. It can fail only after promotion to test rollback.
fn compile_binary(dir: &Path, tag: &str, mode: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let source = dir.join("fixture.rs");
    let binary = dir.join(binary_name());
    let version = tag.trim_start_matches('v');
    std::fs::write(
        &source,
        format!(
            r#"
fn main() {{
    if std::env::args().nth(1).as_deref() == Some("--hold") {{
        std::fs::write(std::env::args().nth(2).unwrap(), "ready").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
        return;
    }}
    assert_eq!(std::env::args().nth(1).as_deref(), Some("--version"));
    println!("hashline {version}");
    let installed = std::env::current_exe().unwrap().file_name().unwrap() == {name:?};
    if {mode:?} == "nonzero" || ({mode:?} == "fail-installed" && installed) {{
        std::process::exit(7);
    }}
}}
"#,
            name = binary_name()
        ),
    )
    .unwrap();
    let output = StdCommand::new("rustc")
        .arg("--crate-name")
        .arg("update_fixture")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile native release fixture");
    assert!(
        output.status.success(),
        "rustc: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

fn build_fixture(tag: &str, dir: &Path) -> PathBuf {
    build_fixture_mode(tag, tag, "ok", dir)
}

/// Package a real executable under the host's release asset name.
fn build_fixture_mode(tag: &str, payload_tag: &str, mode: &str, dir: &Path) -> PathBuf {
    let platform = update::release_platform().expect("host platform has a release asset");
    // Mirrors update.rs archive_ext (kept private there).
    let ext = if platform.starts_with("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let asset_name = format!("hashline-{tag}-{platform}.{ext}");
    let asset_path = dir.join(&asset_name);

    let stage = dir.join("stage");
    let payload = compile_binary(
        &dir.join(format!("payload-{mode}-{payload_tag}")),
        payload_tag,
        mode,
    );
    std::fs::create_dir_all(&stage).expect("create stage dir");
    std::fs::copy(&payload, stage.join(binary_name())).expect("stage real executable");

    if cfg!(windows) {
        let script = format!(
            "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
            stage.join(binary_name()).display(),
            asset_path.display()
        );
        let status = StdCommand::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .expect("run Compress-Archive");
        assert!(status.success(), "Compress-Archive failed");
    } else {
        let status = StdCommand::new("tar")
            .arg("-czf")
            .arg(&asset_path)
            .arg("-C")
            .arg(&stage)
            .arg(binary_name())
            .status()
            .expect("run tar");
        assert!(status.success(), "tar -czf failed");
    }
    asset_path
}

/// Sets an env var for the remainder of the test and removes it on drop.
/// Library tests that touch this var serialize on [`ENV_LOCK`] so they
/// cannot observe each other's mirror; CLI tests always set the var
/// explicitly on the child process.
struct EnvGuard {
    key: &'static str,
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> (Self, std::sync::MutexGuard<'static, ()>) {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: env access is serialized through ENV_LOCK and the value
        // is removed again on drop.
        unsafe { std::env::set_var(key, value) };
        (Self { key }, lock)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(self.key) };
    }
}

// ── CLI end-to-end ───────────────────────────────────────────────────

#[test]
fn cli_check_reports_available_update_as_json() {
    let tag = next_version_tag();
    let home = tempfile::tempdir().expect("temp home");
    let mirror = spawn_mirror(MirrorConfig {
        tag: tag.clone(),
        fixture: None,
        fail_api: false,
    });

    let output = Command::cargo_bin("hashline")
        .expect("hashline binary")
        .args(["update", "--check", "--json"])
        .env("HASHLINE_RELEASES_BASE_URL", &mirror.base)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .expect("run hashline update --check");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(json["success"], serde_json::json!(true));
    assert_eq!(json["status"], serde_json::json!("available"));
    assert_eq!(
        json["installed"],
        serde_json::json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(json["latest"], tag.trim_start_matches('v'));
}

#[test]
fn cli_check_reports_up_to_date_when_current() {
    let home = tempfile::tempdir().expect("temp home");
    let mirror = spawn_mirror(MirrorConfig {
        tag: current_version_tag(),
        fixture: None,
        fail_api: false,
    });

    let output = Command::cargo_bin("hashline")
        .expect("hashline binary")
        .args(["update", "--check", "--json"])
        .env("HASHLINE_RELEASES_BASE_URL", &mirror.base)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .expect("run hashline update --check");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(json["status"], serde_json::json!("up-to-date"));
    assert_eq!(
        json["current"],
        serde_json::json!(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cli_check_fails_cleanly_when_mirror_unreachable() {
    // Reserve a port, then drop the listener so connections are refused.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind dead port");
    let dead_base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);

    let home = tempfile::tempdir().expect("temp home");
    let output = Command::cargo_bin("hashline")
        .expect("hashline binary")
        .args(["update", "--check"])
        .env("HASHLINE_RELEASES_BASE_URL", &dead_base)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .expect("run hashline update --check");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("ERR UPDATE"), "stderr: {stderr}");
    assert!(stderr.contains("HINT"), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "stdout must stay clean");
}

fn temporary_cli(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(binary_name());
    std::fs::copy(assert_cmd::cargo::cargo_bin("hashline"), &path).unwrap();
    path
}

fn run_update(exe: &Path, home: &Path, mirror: &Mirror) -> std::process::Output {
    StdCommand::new(exe)
        .args(["update", "--json"])
        .env("HASHLINE_RELEASES_BASE_URL", &mirror.base)
        .env("HASHLINE_NO_UPDATE_CHECK", "1")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .expect("run temporary CLI self-update")
}

fn assert_version(exe: &Path, tag: &str) {
    let output = StdCommand::new(exe).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("hashline {}", tag.trim_start_matches('v'))
    );
}

#[test]
fn cli_self_update_verifies_bytes_version_and_exact_path_without_touching_other_installs() {
    let temp = tempfile::tempdir().unwrap();
    let tag = next_version_tag();
    let fixture = build_fixture(&tag, temp.path());
    let mirror = spawn_mirror(MirrorConfig {
        tag: tag.clone(),
        fixture: Some(fixture),
        fail_api: false,
    });
    let exe = temporary_cli(&temp.path().join("invoked"));
    let other = temporary_cli(&temp.path().join("other"));
    let old_bytes = std::fs::read(&other).unwrap();
    let output = run_update(&exe, temp.path(), &mirror);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "updated");
    assert_eq!(json["current"], tag.trim_start_matches('v'));
    assert_eq!(
        std::fs::canonicalize(json["path"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&exe).unwrap()
    );
    assert_eq!(
        std::fs::read(&exe).unwrap(),
        std::fs::read(temp.path().join("stage").join(binary_name())).unwrap()
    );
    assert_version(&exe, &tag);
    assert_eq!(std::fs::read(&other).unwrap(), old_bytes);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("other installations are unchanged"));
    assert!(stderr.contains("Restart/reconnect"));
}

#[test]
fn cli_rejects_mismatched_and_nonzero_candidates_without_false_success() {
    for (payload, mode, expected_error) in [
        (current_version_tag(), "ok", "version mismatch"),
        (next_version_tag(), "nonzero", "--version failed"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let tag = next_version_tag();
        let fixture = build_fixture_mode(&tag, &payload, mode, temp.path());
        let mirror = spawn_mirror(MirrorConfig {
            tag: tag.clone(),
            fixture: Some(fixture),
            fail_api: false,
        });
        let exe = temporary_cli(&temp.path().join("invoked"));
        let before = std::fs::read(&exe).unwrap();
        let output = run_update(&exe, temp.path(), &mirror);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("\"status\":\"updated\""));
        assert_eq!(std::fs::read(&exe).unwrap(), before);
        assert_version(&exe, &current_version_tag());
        // This cache means release observed, not successfully installed.
        let cache: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join(".hashline/update-check.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cache["latest_version"], tag.trim_start_matches('v'));
    }
}

#[test]
fn cli_rolls_back_when_installed_version_probe_fails() {
    let temp = tempfile::tempdir().unwrap();
    let tag = next_version_tag();
    let fixture = build_fixture_mode(&tag, &tag, "fail-installed", temp.path());
    let mirror = spawn_mirror(MirrorConfig {
        tag,
        fixture: Some(fixture),
        fail_api: false,
    });
    let exe = temporary_cli(&temp.path().join("invoked"));
    let before = std::fs::read(&exe).unwrap();
    let output = run_update(&exe, temp.path(), &mirror);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("original restored"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\"status\":\"updated\""));
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    assert_version(&exe, &current_version_tag());
}

// ── Library end-to-end ───────────────────────────────────────────────

#[test]
fn library_download_verifies_and_replaces_target() {
    let tag = next_version_tag();
    let scratch = tempfile::tempdir().expect("temp scratch");
    let fixture = build_fixture(&tag, scratch.path());
    let mirror = spawn_mirror(MirrorConfig {
        tag: tag.clone(),
        fixture: Some(fixture),
        fail_api: false,
    });
    let (_env, _lock) = EnvGuard::set("HASHLINE_RELEASES_BASE_URL", &mirror.base);

    let target_dir = tempfile::tempdir().expect("temp target dir");
    let target = target_dir.path().join("bin").join(binary_name());

    let report = update::download_and_install_into(&Release::from_version(&tag), &target)
        .expect("download and install");
    assert_eq!(report.previous, env!("CARGO_PKG_VERSION"));
    assert_eq!(report.current, tag.trim_start_matches('v'));
    assert!(report.checksum_verified, "checksum must be verified");
    assert_eq!(report.path, target);
    assert!(
        report.retained_backup.is_none(),
        "clean install keeps no backup"
    );

    // Verify this test copies a real native executable before the bytes are
    // promoted: run it directly rather than trusting release metadata.
    let output = StdCommand::new(&target)
        .arg("--version")
        .output()
        .expect("execute installed payload");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("hashline {}", tag.trim_start_matches('v'))
    );

    // The API path of fetch_latest_release works against the same mirror.
    let latest = update::fetch_latest_release().expect("fetch latest release");
    assert_eq!(latest.version, tag.trim_start_matches('v'));
}

#[cfg(windows)]
struct RunningFixture(std::process::Child);

#[cfg(windows)]
impl RunningFixture {
    fn start(exe: &Path, ready: &Path) -> Self {
        let child = StdCommand::new(exe)
            .arg("--hold")
            .arg(ready)
            .spawn()
            .unwrap();
        let fixture = Self(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        fixture
    }
}

#[cfg(windows)]
impl Drop for RunningFixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
#[test]
fn windows_repeated_update_with_running_images_and_legacy_backup() {
    let temp = tempfile::tempdir().unwrap();
    let tag = next_version_tag();
    let fixture = build_fixture(&tag, temp.path());
    let mirror = spawn_mirror(MirrorConfig {
        tag: tag.clone(),
        fixture: Some(fixture),
        fail_api: false,
    });
    let (_env, _lock) = EnvGuard::set("HASHLINE_RELEASES_BASE_URL", &mirror.base);
    let target = compile_binary(&temp.path().join("installed"), &current_version_tag(), "ok");
    let legacy = target.with_extension("exe.old");
    std::fs::copy(&target, &legacy).unwrap();
    let legacy_bytes = std::fs::read(&legacy).unwrap();
    let _legacy_process = RunningFixture::start(&legacy, &temp.path().join("legacy-ready"));
    let _first_process = RunningFixture::start(&target, &temp.path().join("first-ready"));
    let first = update::download_and_install_into(&Release::from_version(&tag), &target).unwrap();
    assert_version(&target, &tag);
    let _second_process = RunningFixture::start(&target, &temp.path().join("second-ready"));
    let second = update::download_and_install_into(&Release::from_version(&tag), &target).unwrap();
    assert_version(&target, &tag);
    assert_eq!(std::fs::read(&legacy).unwrap(), legacy_bytes);
    // Windows may allow delete-pending images, so retained files are optional;
    // if both remain, they must belong to distinct attempts.
    if let (Some(first), Some(second)) = (first.retained_backup, second.retained_backup) {
        assert_ne!(first, second);
    }
}

#[cfg(windows)]
#[test]
fn windows_retirement_failure_preserves_target_and_returns_error() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let tag = next_version_tag();
    let fixture = build_fixture(&tag, temp.path());
    let mirror = spawn_mirror(MirrorConfig {
        tag: tag.clone(),
        fixture: Some(fixture),
        fail_api: false,
    });
    let (_env, _lock) = EnvGuard::set("HASHLINE_RELEASES_BASE_URL", &mirror.base);
    let target = temporary_cli(&temp.path().join("installed"));
    let before = std::fs::read(&target).unwrap();
    // FILE_SHARE_READ but deliberately no FILE_SHARE_DELETE: a real OS-level
    // retirement failure, not a mocked rename result.
    let _locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&target)
        .unwrap();
    let error =
        update::download_and_install_into(&Release::from_version(&tag), &target).unwrap_err();
    assert!(error.contains("cannot retire"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
        1,
        "failed attempt should be cleaned"
    );
}

#[test]
fn library_fetch_falls_back_to_redirect_when_api_fails() {
    let tag = next_version_tag();
    let expected_version = tag.trim_start_matches('v').to_string();
    let mirror = spawn_mirror(MirrorConfig {
        tag,
        fixture: None,
        fail_api: true,
    });
    let (_env, _lock) = EnvGuard::set("HASHLINE_RELEASES_BASE_URL", &mirror.base);

    let latest = update::fetch_latest_release().expect("redirect fallback");
    assert_eq!(
        latest.tag,
        format!("v{expected_version}"),
        "tag must come from the /latest redirect target"
    );
    assert_eq!(latest.version, expected_version);
}
