use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

const HASH_DIR: &str = ".hashline/hashes";
// LHH2: hash function now strips trailing whitespace before hashing
// (formerly LHH1 hashed raw line content, breaking anchors after
// formatter runs).
// LHH3: content_hash widened to u64 (xxh3) for collision resistance
// at no extra CPU cost.
const MAGIC: &[u8] = b"LHH3";
const HDR_MAGIC: usize = 4;
const HDR_FIXED: usize = 1 + 8 + 8 + 8 + 4; // version + mtime + size + content_hash + line_count

pub struct HashSidecar {
    pub mtime_secs: u64,
    pub size: u64,
    pub content_hash: u64,
    pub short_hashes: Vec<u8>,
}

impl HashSidecar {
    pub fn store_path(root: &Path, source: &Path) -> PathBuf {
        let relative = source.strip_prefix(root).unwrap_or(source);
        let mut path = root.join(HASH_DIR).join(relative);
        path.set_extension("lhhash");
        path
    }

    pub fn ensure_dir(root: &Path, source: &Path) -> io::Result<()> {
        let path = Self::store_path(root, source);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    pub fn write(&self, root: &Path, source: &Path) -> io::Result<()> {
        Self::ensure_dir(root, source)?;
        let path = Self::store_path(root, source);

        let mut temp = NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;

        let hdr = HDR_MAGIC + HDR_FIXED;
        let mut buf = Vec::with_capacity(hdr + self.short_hashes.len());
        buf.extend_from_slice(MAGIC);
        buf.push(1);
        buf.extend_from_slice(&self.mtime_secs.to_le_bytes());
        buf.extend_from_slice(&self.size.to_le_bytes());
        buf.extend_from_slice(&self.content_hash.to_le_bytes());
        buf.extend_from_slice(&(self.short_hashes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.short_hashes);

        temp.write_all(&buf)?;
        temp.flush()?;
        // No fsync: the sidecar is a regeneratable cache (mtime + size +
        // content_hash invalidates on any change), so durability across a
        // crash is unnecessary. Skipping the fsync trims ~5-10 ms off the
        // cold-cache path on most filesystems.
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn read(root: &Path, source: &Path) -> io::Result<Self> {
        let path = Self::store_path(root, source);
        let mut file = fs::File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        if buf.len() < HDR_MAGIC + HDR_FIXED || &buf[0..4] != b"LHH3" {
            return Err(io::Error::other("invalid hash sidecar"));
        }

        let _version = buf[4];
        let off = HDR_MAGIC + 1;
        let mtime_secs = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
        let size = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());
        let content_hash = u64::from_le_bytes(buf[off + 16..off + 24].try_into().unwrap());
        let line_count =
            u32::from_le_bytes(buf[off + 24..off + 28].try_into().unwrap()) as usize;
        let hdr_end = off + 28;

        if buf.len() < hdr_end + line_count {
            return Err(io::Error::other("truncated hash sidecar"));
        }

        let short_hashes = buf[hdr_end..hdr_end + line_count].to_vec();
        Ok(Self {
            mtime_secs,
            size,
            content_hash,
            short_hashes,
        })
    }

    pub fn exists(root: &Path, source: &Path) -> bool {
        Self::store_path(root, source).exists()
    }

    #[allow(dead_code)]
    pub fn invalidate(root: &Path, source: &Path) -> io::Result<()> {
        let path = Self::store_path(root, source);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Walk up from `path`'s parent directory looking for a project root marker
/// (`.git`, `.hg`, `Cargo.toml`, `package.json`, `pyproject.toml`,
/// `go.mod`, `.hashline`). Falls back to the file's parent directory if no
/// marker is found within 16 levels — this matches the agent workflow where
/// the file usually lives inside a repo but graceful degradation matters
/// when invoked on standalone files.
pub fn discover_sidecar_root(path: &Path) -> PathBuf {
    const MARKERS: &[&str] = &[
        ".hashline",
        ".git",
        ".hg",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];

    let start = path.parent().unwrap_or(path);
    let mut current = start;
    for _ in 0..16 {
        for marker in MARKERS {
            if current.join(marker).exists() {
                return current.to_path_buf();
            }
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    start.to_path_buf()
}
