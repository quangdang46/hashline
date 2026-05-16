use std::fs;
use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;

use crate::document::Document;
use crate::error::LinehashError;

pub fn check_guard(
    doc: &Document,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
) -> Result<(), LinehashError> {
    let Some(meta) = &doc.file_meta else {
        return Ok(());
    };

    if expect_mtime.is_some_and(|expected| expected != meta.mtime_secs)
        || expect_inode.is_some_and(|expected| expected != meta.inode)
    {
        return Err(LinehashError::StaleFile {
            path: doc.path.display().to_string(),
        });
    }

    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), LinehashError> {
    atomic_write_with(path, |file| file.write_all(bytes))
}

pub fn atomic_write_document(path: &Path, doc: &Document) -> Result<(), LinehashError> {
    atomic_write_with(path, |file| doc.write_to(file))
}

pub fn atomic_write_with<F>(path: &Path, write_contents: F) -> Result<(), LinehashError>
where
    F: FnOnce(&mut fs::File) -> io::Result<()>,
{
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
fn persist_with_retry(mut temp: NamedTempFile, path: &Path) -> Result<(), LinehashError> {
    use std::io::Read;
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
                    // This loses atomicity but avoids the rename race.
                    let mut buf = Vec::new();
                    temp.as_file_mut().rewind()?;
                    temp.as_file_mut().read_to_end(&mut buf)?;
                    drop(temp);
                    // Retry direct write a few times too
                    for write_attempt in 0..5 {
                        match fs::write(path, &buf) {
                            Ok(()) => return Ok(()),
                            Err(e) if write_attempt < 4 => {
                                thread::sleep(Duration::from_millis(50));
                            }
                            Err(e) => return Err(LinehashError::Io(e)),
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
fn persist_with_retry(temp: NamedTempFile, path: &Path) -> Result<(), LinehashError> {
    temp.persist(path)
        .map_err(|error| LinehashError::Io(error.error))?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), LinehashError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), LinehashError> {
    Ok(())
}
