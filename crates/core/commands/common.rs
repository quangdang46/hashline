use std::fs;
use std::io::Write;
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

    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;

    temp.persist(path)
        .map_err(|error| LinehashError::Io(error.error))?;

    sync_parent_directory(parent)?;
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
