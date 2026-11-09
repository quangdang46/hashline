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
    const NAMES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
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
fn reject_special_write_path(path: &Path) -> Result<(), HashlineError> {
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
fn reject_special_write_path(_path: &Path) -> Result<(), HashlineError> {
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

/// Temp files created by [`atomic_write_with`] via `NamedTempFile` use a
/// `.tmp` prefix in the target directory. If the process is killed
/// (SIGKILL, power loss) they leak. Sweep entries older than
/// [`STALE_TEMP_AGE`] so in-flight writes from concurrent processes —
/// whose temp files are always fresh — are never touched.
const STALE_TEMP_AGE: Duration = Duration::from_secs(600);

/// Best-effort removal of stale `.tmp*` files in `dir`. All errors are
/// ignored: a failed sweep must never fail the write it precedes.
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
        if !name.starts_with(".tmp") {
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

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), HashlineError> {
    atomic_write_with(path, |file| file.write_all(bytes))
}

/// Fast variant of atomic_write that skips fsync and uses a simple
/// read-modify-write instead of temp-file + rename. Suitable for agent
/// use cases where crash safety is not critical (the agent can always
/// re-read and re-edit on failure).
pub fn fast_write(path: &Path, bytes: &[u8]) -> Result<(), HashlineError> {
    use std::io::Write;
    reject_special_write_path(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)?;
    file.write_all(bytes)?;
    // No sync_all / sync_parent_directory call.
    Ok(())
}

pub fn atomic_write_with<F>(path: &Path, write_contents: F) -> Result<(), HashlineError>
where
    F: FnOnce(&mut fs::File) -> io::Result<()>,
{
    reject_special_write_path(path)?;
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

    // Reclaim `.tmp*` files leaked by previously killed atomic writes
    // before creating our own temp file in the same directory.
    sweep_stale_temp_files(parent);

    let mut temp = NamedTempFile::new_in(parent)?;
    if let Some(permissions) = existing_permissions {
        temp.as_file().set_permissions(permissions)?;
    }

    write_contents(temp.as_file_mut())?;
    temp.flush()?;
    temp.as_file().sync_all()?;

    // On Windows, antivirus or the OS may briefly hold a lock on the target
    // file after a previous write. Retry the persist a few times to work
    // around transient ACCESS_DENIED errors.
    persist_with_retry(temp, path)?;

    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(windows)]
fn persist_with_retry(mut temp: NamedTempFile, path: &Path) -> Result<(), HashlineError> {
    use std::io::Read;
    use std::io::Seek;
    use std::thread;
    use std::time::Duration;

    // Retry persist up to 5 times with increasing backoff.
    // On Windows, antivirus/Defender may briefly lock the target file after
    // it was created/written, causing MoveFileExW to fail with ACCESS_DENIED.
    for attempt in 0..10 {
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
fn persist_with_retry(temp: NamedTempFile, path: &Path) -> Result<(), HashlineError> {
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
    fn atomic_write_sweeps_stale_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");

        // A fresh `.tmp*` file simulates an in-flight write: must survive.
        let fresh = dir.path().join(".tmpfresh123");
        fs::write(&fresh, "in-flight").unwrap();
        // A `.tmp*` file with an old mtime simulates a SIGKILL leftover.
        let stale = dir.path().join(".tmpstale456");
        fs::write(&stale, "leftover").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
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
        assert!(keep.exists(), "non-.tmp-prefixed file must be preserved");
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
