use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tempfile::NamedTempFile;

use crate::error::HashlineError;

/// Interpret a small set of C-style escape sequences in `input`.
///
/// Supported sequences: `\n`, `\r`, `\t`, `\0`, `\\`, `\"`, `\'`.
/// Any unrecognized escape (e.g. `\q`) is left as-is so that callers do not
/// silently lose bytes when feeding arbitrary user content. A trailing
/// backslash is also preserved literally.
pub fn interpret_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Best-effort canonicalization that also works for paths that do not
/// exist yet: when the full path cannot be canonicalized, the parent
/// directory is canonicalized and the file name is re-attached. Returns
/// the underlying I/O error only when neither the path nor its parent
/// can be resolved.
pub fn canonicalize_lenient(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let file_name = path.file_name().ok_or(err)?;
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            let canonical_parent = fs::canonicalize(parent)?;
            Ok(canonical_parent.join(file_name))
        }
        Err(err) => Err(err),
    }
}

/// Returns `true` when two paths resolve to the same filesystem location.
///
/// Tolerant of `.`/`..` components, symlinks, and — on Windows — case
/// differences and the OS stripping of trailing dots/spaces from name
/// components (so `foo.` and `foo` compare equal, matching what the
/// filesystem will actually do).
pub fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ca), Ok(cb)) = (canonicalize_lenient(a), canonicalize_lenient(b)) else {
        return false;
    };
    if ca == cb {
        return true;
    }
    normalized_path_string(&ca) == normalized_path_string(&cb)
}

/// Canonical-path comparison key. On Windows the filesystem is
/// case-insensitive and strips trailing dots/spaces from each name
/// component, so normalize those away before comparing. On other
/// platforms this is just the lossy path string.
fn normalized_path_string(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .split(['\\', '/'])
            .map(|component| component.trim_end_matches(['.', ' ']).to_lowercase())
            .collect::<Vec<_>>()
            .join("\\")
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

/// `true` when `name` is a DOS device name reserved on Windows
/// (CON, PRN, AUX, NUL, COM1-9, LPT1-9), ignoring any extension —
/// `NUL.txt` is just as reserved as `NUL`.
#[cfg(windows)]
pub fn is_reserved_device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    const NAMES: [&str; 24] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "CONIN$",
        "CONOUT$",
    ];
    NAMES
        .iter()
        .any(|reserved| base.eq_ignore_ascii_case(reserved))
}

/// Reject paths that cannot name a regular file on this platform:
/// Windows DOS device names (`NUL`, `CON`, ...) in any directory and
/// `\\.\` / `\\?\`-style device-namespace prefixes. Writing to these
/// "succeeds" but lands nowhere (or worse), while still reporting OK.
#[cfg(windows)]
pub(crate) fn reject_special_write_path(path: &Path) -> Result<(), HashlineError> {
    use std::path::{Component, Prefix};

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if is_reserved_device_name(name) {
            return Err(HashlineError::InvalidPath {
                path: path.display().to_string(),
                reason: format!("'{name}' is a reserved device name"),
            });
        }
    }
    for component in path.components() {
        if let Component::Prefix(prefix) = component {
            if matches!(prefix.kind(), Prefix::DeviceNS(_) | Prefix::Verbatim(_)) {
                return Err(HashlineError::InvalidPath {
                    path: path.display().to_string(),
                    reason: "device or verbatim path prefix is not a regular file".into(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn reject_special_write_path(_path: &Path) -> Result<(), HashlineError> {
    Ok(())
}

/// Post-write sanity check: `path` must now be a regular file whose
/// length matches the bytes we wrote. Catches writes that silently
/// went to a device or were redirected by filesystem name
/// normalization, so a command never reports OK for a write that did
/// not land where intended.
pub fn verify_written_file(path: &Path, expected_len: usize) -> Result<(), HashlineError> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() {
        return Err(HashlineError::InvalidPath {
            path: path.display().to_string(),
            reason: "target is not a regular file after write".into(),
        });
    }
    if meta.len() != expected_len as u64 {
        return Err(HashlineError::InvalidPath {
            path: path.display().to_string(),
            reason: format!(
                "expected {expected_len} bytes on disk after write, found {}",
                meta.len()
            ),
        });
    }
    Ok(())
}

/// Filename prefix for temp files created by [`atomic_write_with`]. The
/// prefix is hashline-specific (`.hashline.tmp.`) so the stale-temp sweep
/// can never mistake another tool's `.tmp*` files for ours — the old
/// generic `.tmp` prefix deleted unrelated files older than
/// [`STALE_TEMP_AGE`] in the target directory (bd-temp-sweep-foreign-files-
/// ihp0).
const TEMP_PREFIX: &str = ".hashline.tmp.";

/// Temp files created by [`atomic_write_with`] via `NamedTempFile` use the
/// [`TEMP_PREFIX`] prefix in the target directory. If the process is
/// killed (SIGKILL, power loss) they leak. Sweep entries older than
/// [`STALE_TEMP_AGE`] so in-flight writes from concurrent processes —
/// whose temp files are always fresh — are never touched.
const STALE_TEMP_AGE: Duration = Duration::from_secs(600);

/// Best-effort removal of stale [`TEMP_PREFIX`]* files in `dir`. Only
/// files carrying the hashline-specific prefix are candidates — foreign
/// `.tmp*` files belonging to other tools are never touched. All errors
/// are ignored: a failed sweep must never fail the write it precedes.
fn sweep_stale_temp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let Ok(now) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(modified_secs) = modified.duration_since(SystemTime::UNIX_EPOCH) else {
            continue;
        };
        if now.saturating_sub(modified_secs) < STALE_TEMP_AGE {
            continue;
        }
        let _ = fs::remove_file(entry.path());
    }
}

/// Pre-flight validation shared by CLI `remove` and MCP `remove_file`
/// (bd-mcp-serve-parity-batch-nh8w): reject device/reserved paths, require
/// the path to exist, and refuse directories — the MCP tool must not be a
/// back door around the CLI's guards.
pub fn validate_remove_path(path: &Path) -> Result<(), HashlineError> {
    reject_special_write_path(path)?;
    // `symlink_metadata`, not `metadata`: a symlink is removable even when
    // it points at a directory — only a real directory is refused.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Err(HashlineError::IsDirectory {
            path: path.display().to_string(),
        }),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(HashlineError::FileNotFound {
            path: path.display().to_string(),
        }),
        Err(e) => Err(HashlineError::Io(e)),
    }
}

/// Pre-flight validation shared by CLI `rename` and MCP `rename_file`
/// (bd-mcp-serve-parity-batch-nh8w): device/reserved paths, source must be
/// an existing regular file (never a directory — `rename` must not move a
/// tree), src/dst must not resolve to the same file, and the destination
/// must not exist unless `overwrite_ok` (CLI `--force`). The caller still
/// performs parent creation and the actual `fs::rename`; on Unix `--force`
/// needs no pre-remove because `rename(2)` replaces atomically.
pub fn validate_rename_paths(
    src: &Path,
    dst: &Path,
    overwrite_ok: bool,
) -> Result<(), HashlineError> {
    reject_special_write_path(src)?;
    reject_special_write_path(dst)?;

    if !src.exists() {
        return Err(HashlineError::FileNotFound {
            path: src.display().to_string(),
        });
    }
    // `metadata` follows symlinks, so a symlink to a regular file is still
    // allowed while directories and junctions (which report as dirs) are
    // refused.
    let src_meta = fs::metadata(src)?;
    if src_meta.is_dir() {
        return Err(HashlineError::IsDirectory {
            path: src.display().to_string(),
        });
    }
    if !src_meta.is_file() {
        return Err(HashlineError::InvalidPath {
            path: src.display().to_string(),
            reason: "source is not a regular file".into(),
        });
    }

    // Guard against src == dst BEFORE any destructive step. Aliases like
    // `./f`, `dir/../f`, case variants, and Windows trailing dot/space
    // names all resolve to the same file; without this check `--force`
    // would delete the source and then fail the rename.
    if paths_refer_to_same_file(src, dst) {
        return Err(HashlineError::SameFile {
            path: src.display().to_string(),
        });
    }

    if dst.exists() {
        // A directory destination can never be replaced by rename; fail
        // cleanly instead of surfacing a remove_file I/O error.
        if fs::metadata(dst).map(|m| m.is_dir()).unwrap_or(false) {
            return Err(HashlineError::IsDirectory {
                path: dst.display().to_string(),
            });
        }
        if !overwrite_ok {
            return Err(HashlineError::TargetExists {
                path: dst.display().to_string(),
            });
        }
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), HashlineError> {
    atomic_write_with(path, |file| file.write_all(bytes))
}

/// Byte-exact post-write verification: re-reads `path` and compares the
/// on-disk bytes against `expected`.
///
/// Stronger than [`verify_written_file`]'s length check — it catches
/// truncated/torn writes, silent redirects, and (for callers that just
/// wrote) concurrent overwrites that landed between the write and this
/// check. On mismatch returns [`HashlineError::ConcurrentModification`]
/// so the caller fails loudly instead of reporting a silent OK; I/O
/// errors reading back propagate as [`HashlineError::Io`].
pub fn verify_written_bytes(path: &Path, expected: &[u8]) -> Result<(), HashlineError> {
    let actual = fs::read(path)?;
    if actual == expected {
        return Ok(());
    }
    Err(HashlineError::ConcurrentModification {
        path: path.display().to_string(),
    })
}

/// Fast variant of atomic_write that skips fsync and uses a simple
/// read-modify-write instead of temp-file + rename. Suitable for agent
/// use cases where crash safety is not critical (the agent can always
/// re-read and re-edit on failure).
///
/// Residual races (documented, not fully fixable without OS-level locks):
/// - **Torn reads:** truncate-then-write exposes 0-byte/partial states to
///   concurrent readers. Use [`atomic_write`] (`--safe`) when readers must
///   never observe a torn file.
/// - **Lost updates:** two writers can interleave; callers that care run
///   [`verify_written_bytes`] afterwards and surface
///   [`HashlineError::ConcurrentModification`] instead of a silent OK.
/// - Writes go through the existing inode, so hardlinked names all see the
///   new content (unlike [`atomic_write`], which replaces the directory
///   entry and leaves other links on the old content).
pub fn fast_write(path: &Path, bytes: &[u8]) -> Result<(), HashlineError> {
    use std::io::Write;
    reject_special_write_path(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)?;
    file.write_all(bytes)?;
    // Flush data to stable storage. `sync_data` (not `sync_all`) is enough:
    // fast_write mutates the existing inode in place — there is no new
    // directory entry to persist — and the cost is one fdatasync, far
    // cheaper than the temp+rename+dir-fsync of `atomic_write` while still
    // preventing silent data loss on power failure.
    file.sync_data()?;
    Ok(())
}

/// Resolve `path` through any symlink chain so an atomic write lands on
/// the *target* — matching `fast_write`, which follows symlinks via
/// `OpenOptions`. Without this, `temp.persist(path)` would replace the
/// symlink itself with a regular file, silently changing what the path
/// names (bd-write-safety-batch-sz62).
///
/// Follows up to 40 links (the kernel's own ELOOP bound); a dangling
/// final component is returned as-is so the write can create it, exactly
/// like `OpenOptions::create` would.
fn resolve_symlink_for_write(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&current)?;
                current = if target.is_absolute() {
                    target
                } else {
                    match current.parent() {
                        Some(parent) if !parent.as_os_str().is_empty() => parent.join(&target),
                        _ => target,
                    }
                };
            }
            Ok(_) => return Ok(current),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "too many levels of symbolic links: {}",
        path.display()
    )))
}

pub fn atomic_write_with<F>(path: &Path, write_contents: F) -> Result<(), HashlineError>
where
    F: FnOnce(&mut fs::File) -> io::Result<()>,
{
    atomic_write_with_check(path, write_contents, || Ok(()))
}

/// Like [`atomic_write_with`], but runs `pre_persist` immediately before
/// **each** persist (rename) attempt — including every retry inside the
/// Windows persist backoff loop.
///
/// This shrinks the check→persist TOCTOU window for read-modify-write
/// callers (e.g. `patch --safe`): the drift check that ran before the
/// (potentially slow) temp-file write/fsync is repeated right before the
/// rename, so a file swapped in during temp-write is caught instead of
/// being silently overwritten by the persist. The remaining window is a
/// single rename syscall — closing it fully would need an OS-level lock,
/// which hashline deliberately does not take.
///
/// `pre_persist` errors abort the write; the temp file is dropped
/// (deleted) and the target is left untouched.
pub fn atomic_write_with_check<F, G>(
    path: &Path,
    write_contents: F,
    pre_persist: G,
) -> Result<(), HashlineError>
where
    F: FnOnce(&mut fs::File) -> io::Result<()>,
    G: Fn() -> Result<(), HashlineError>,
{
    reject_special_write_path(path)?;
    // Write through symlinks to the resolved target so `--safe` has the
    // same follow-symlink semantics as `fast_write` — otherwise persist
    // would replace the link itself with a regular file.
    let resolved = resolve_symlink_for_write(path)?;
    let path = resolved.as_path();
    // For a bare relative path like "sample.js", `path.parent()` returns
    // `Some(Path::new(""))` rather than `None`, so the `unwrap_or` fallback to
    // "." never fires. The empty path then fails when we try to open it for
    // fsync. Normalize empty parents to "." so we always have a directory we
    // can `fs::File::open` for `sync_all`.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let existing_permissions = fs::metadata(path).ok().map(|meta| meta.permissions());

    // Reclaim stale `.hashline.tmp.*` files leaked by previously killed
    // atomic writes before creating our own temp file in the same dir.
    sweep_stale_temp_files(parent);

    let mut builder = tempfile::Builder::new();
    builder.prefix(TEMP_PREFIX);
    // New files must get normal creation permissions (0666 & ~umask —
    // typically 0644), not NamedTempFile's restrictive 0600 default, so an
    // atomic write produces the same mode `fast_write`/`OpenOptions`
    // would. `Builder::permissions` still passes through the process
    // umask at open time. Existing files keep their current mode via
    // `existing_permissions` below.
    #[cfg(unix)]
    if existing_permissions.is_none() {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temp = builder.tempfile_in(parent)?;
    if let Some(permissions) = existing_permissions {
        temp.as_file().set_permissions(permissions)?;
    }

    write_contents(temp.as_file_mut())?;
    temp.flush()?;
    temp.as_file().sync_all()?;

    // On Windows, antivirus or the OS may briefly hold a lock on the target
    // file after a previous write. Retry the persist a few times to work
    // around transient ACCESS_DENIED errors. `pre_persist` re-validates the
    // target before every attempt so a drifted/swapped file is not
    // clobbered by a late retry.
    persist_with_retry(temp, path, &pre_persist)?;

    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(windows)]
fn persist_with_retry<G>(
    mut temp: NamedTempFile,
    path: &Path,
    pre_persist: &G,
) -> Result<(), HashlineError>
where
    G: Fn() -> Result<(), HashlineError>,
{
    use std::io::Read;
    use std::io::Seek;
    use std::thread;
    use std::time::Duration;

    // Retry persist up to 5 times with increasing backoff.
    // On Windows, antivirus/Defender may briefly lock the target file after
    // it was created/written, causing MoveFileExW to fail with ACCESS_DENIED.
    for attempt in 0..10 {
        // Re-validate the target before every attempt: the backoff sleeps
        // widen the drift window, and a check that only ran once before the
        // loop would miss a swap landing between attempts. A `pre_persist`
        // error aborts the whole write — it must NOT fall through to the
        // non-atomic fallback, which would clobber the drifted file.
        pre_persist()?;
        match temp.persist(path) {
            Ok(_) => return Ok(()),
            Err(err) => {
                temp = err.file;
                if attempt < 9 {
                    thread::sleep(Duration::from_millis(
                        10 * 2u64.saturating_pow(attempt as u32),
                    ));
                } else {
                    // Last resort: read temp file contents and write directly.
                    // This loses atomicity — a concurrent reader can observe
                    // a torn file — so surface a warning instead of silently
                    // degrading while still reporting success.
                    eprintln!(
                        "warning: atomic persist of '{}' failed after retries; \
                         falling back to a non-atomic write",
                        path.display()
                    );
                    let mut buf = Vec::new();
                    temp.as_file_mut().seek(std::io::SeekFrom::Start(0))?;
                    temp.as_file_mut().read_to_end(&mut buf)?;
                    drop(temp);
                    // The fallback is non-atomic AND bypasses the rename —
                    // re-run the drift check so it cannot clobber a file
                    // that changed while persist was retrying.
                    pre_persist()?;
                    // Retry direct write a few times too
                    for write_attempt in 0..5 {
                        match fs::write(path, &buf) {
                            Ok(()) => return Ok(()),
                            Err(_) if write_attempt < 4 => {
                                thread::sleep(Duration::from_millis(50));
                            }
                            Err(e) => return Err(HashlineError::Io(e)),
                        }
                    }
                    unreachable!()
                }
            }
        }
    }
    unreachable!()
}

#[cfg(not(windows))]
fn persist_with_retry<G>(
    temp: NamedTempFile,
    path: &Path,
    pre_persist: &G,
) -> Result<(), HashlineError>
where
    G: Fn() -> Result<(), HashlineError>,
{
    pre_persist()?;
    temp.persist(path)
        .map_err(|error| HashlineError::Io(error.error))?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), HashlineError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), HashlineError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_detects_dot_and_dotdot_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();
        // `..` only resolves through an existing directory.
        fs::create_dir(dir.path().join("sub")).unwrap();

        assert!(paths_refer_to_same_file(&file, &file));
        assert!(paths_refer_to_same_file(
            &file,
            &dir.path().join(".").join("f.txt")
        ));
        assert!(paths_refer_to_same_file(
            &file,
            &dir.path().join("sub").join("..").join("f.txt")
        ));
        // A different file in the same directory is not the same file.
        let other = dir.path().join("g.txt");
        fs::write(&other, "y").unwrap();
        assert!(!paths_refer_to_same_file(&file, &other));
    }

    #[test]
    fn same_file_handles_nonexistent_destination() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();
        let missing = dir.path().join("missing.txt");

        assert!(!paths_refer_to_same_file(&file, &missing));
        // canonicalize_lenient still resolves the parent for missing paths.
        let lenient = canonicalize_lenient(&missing).unwrap();
        assert_eq!(lenient.file_name().unwrap(), "missing.txt");
    }

    #[test]
    fn verify_written_file_checks_length() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();

        assert!(verify_written_file(&file, 5).is_ok());
        assert!(matches!(
            verify_written_file(&file, 3),
            Err(HashlineError::InvalidPath { .. })
        ));
        assert!(matches!(
            verify_written_file(&dir.path().join("nope.txt"), 0),
            Err(HashlineError::Io(_))
        ));
    }

    #[test]
    fn verify_written_bytes_detects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();

        assert!(verify_written_bytes(&file, b"hello").is_ok());
        // Same length, different bytes — a length-only check would miss
        // this; the byte comparison must not.
        let err = verify_written_bytes(&file, b"world").unwrap_err();
        assert_eq!(err.kind(), "CONCURRENT_MODIFICATION");
        assert_eq!(err.report_url(), None);
        // Missing file surfaces as Io, not a false OK.
        assert!(matches!(
            verify_written_bytes(&dir.path().join("nope.txt"), b""),
            Err(HashlineError::Io(_))
        ));
    }

    #[test]
    fn atomic_write_with_check_runs_pre_persist_and_aborts_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "original\n").unwrap();

        // A failing pre_persist check must abort the write entirely: the
        // target keeps its content and no temp file is persisted.
        let err = atomic_write_with_check(
            &target,
            |f| f.write_all(b"new\n"),
            || {
                Err(HashlineError::StaleFile {
                    path: target.display().to_string(),
                })
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), "STALE_FILE");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");

        // A passing check lets the write through.
        atomic_write_with_check(&target, |f| f.write_all(b"new\n"), || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new\n");
    }

    #[test]
    fn atomic_write_sweeps_stale_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");

        // A fresh temp file simulates an in-flight write: must survive.
        let fresh = dir.path().join(".hashline.tmp.fresh123");
        fs::write(&fresh, "in-flight").unwrap();
        // A temp file with an old mtime simulates a SIGKILL leftover.
        let stale = dir.path().join(".hashline.tmp.stale456");
        fs::write(&stale, "leftover").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        // A foreign `.tmp*` file from another tool must NEVER be swept,
        // even when old (bd-temp-sweep-foreign-files-ihp0).
        let foreign = dir.path().join(".tmpforeign789");
        fs::write(&foreign, "other-tool").unwrap();
        let foreign_handle = fs::File::options().write(true).open(&foreign).unwrap();
        foreign_handle.set_modified(old).unwrap();
        drop(foreign_handle);
        // Non-temp files must never be touched.
        let keep = dir.path().join("keep.tmp.txt");
        fs::write(&keep, "keep").unwrap();
        let keep_handle = fs::File::options().write(true).open(&keep).unwrap();
        keep_handle.set_modified(old).unwrap();
        drop(keep_handle);

        atomic_write(&target, b"data").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "data");
        assert!(fresh.exists(), "fresh temp file must be preserved");
        assert!(!stale.exists(), "stale temp file must be swept");
        assert!(foreign.exists(), "foreign .tmp* file must never be swept");
        assert!(keep.exists(), "non-temp file must be preserved");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_symlink_to_target() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "original\n").unwrap();
        // Give the target a distinctive mode so we can also verify the
        // symlink target's permissions are preserved, not the link's.
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        atomic_write(&link, b"through the link\n").unwrap();

        // The write must land on the TARGET; the symlink must remain a
        // symlink pointing at it (persist must not replace the link
        // itself with a regular file).
        assert_eq!(fs::read_to_string(&target).unwrap(), "through the link\n");
        let meta = fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink(), "link must stay a symlink");
        assert_eq!(
            fs::metadata(&link).unwrap().permissions().mode() & 0o777,
            0o640,
            "target permissions must be preserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_new_file_gets_normal_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fresh.txt");

        atomic_write(&target, b"new\n").unwrap();

        // A brand-new file must get 0666 & ~umask (typically 0644), NOT
        // NamedTempFile's restrictive 0600 — parity with fast_write's
        // OpenOptions::create semantics (bd-write-safety-batch-sz62).
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "expected 0644 under a standard umask");
    }

    #[cfg(windows)]
    #[test]
    fn reserved_device_names_detected() {
        for name in ["NUL", "nul.txt", "CON", "aux", "com1", "LPT9.log", "PRN"] {
            assert!(is_reserved_device_name(name), "{name} should be reserved");
        }
        for name in ["null", "console", "com10", "lpt0", "file.txt", "NULX"] {
            assert!(
                !is_reserved_device_name(name),
                "{name} should not be reserved"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn fast_write_rejects_device_names() {
        let dir = tempfile::tempdir().unwrap();
        let nul = dir.path().join("NUL");
        assert!(matches!(
            fast_write(&nul, b"x"),
            Err(HashlineError::InvalidPath { .. })
        ));
        assert!(matches!(
            atomic_write(&nul, b"x"),
            Err(HashlineError::InvalidPath { .. })
        ));
    }
}
