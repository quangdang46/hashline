//! Persistence layer for trigram indexes.
//!
//! Manages writing and reading index files from `.linehash/indexes/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::search::index::{compute_content_hash, IndexBuilder};
use crate::search::types::{IndexMeta, MmapIndex, TrigramIndex};

const INDEX_DIR: &str = ".linehash/indexes";

pub struct IndexStore {
    root: PathBuf,
}

impl IndexStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn index_path(&self, source_file: &Path) -> PathBuf {
        let relative = source_file.strip_prefix(&self.root).unwrap_or(source_file);
        let mut path = self.root.join(INDEX_DIR).join(relative);
        path.set_extension("lhidx");
        path
    }

    pub fn ensure_dir(&self, source_file: &Path) -> std::io::Result<()> {
        let index_path = self.index_path(source_file);
        if let Some(parent) = index_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    pub fn write_index(
        &self,
        source_file: &Path,
        content: &[u8],
        mtime: u64,
    ) -> std::io::Result<TrigramIndex> {
        self.ensure_dir(source_file)?;

        let index_path = self.index_path(source_file);
        let size = content.len() as u64;
        let hash = compute_content_hash(content);

        let mut builder = IndexBuilder::new();
        for (line_idx, line) in content.split(|&b| b == b'\n').enumerate() {
            builder.add_line(line_idx, line);
        }

        let (index, meta) = builder.build_with_meta(mtime, size, hash);
        index.write_to_file(&index_path, &meta)?;

        Ok(index)
    }

    pub fn read_index(&self, source_file: &Path) -> std::io::Result<(MmapIndex, IndexMeta)> {
        let index_path = self.index_path(source_file);
        TrigramIndex::read_from_file(&index_path)
    }

    pub fn is_stale(&self, source_file: &Path, content: &[u8], mtime: u64) -> bool {
        let Ok((_, meta)) = self.read_index(source_file) else {
            return true;
        };

        let size = content.len() as u64;
        let hash = compute_content_hash(content);
        // Count lines same way IndexBuilder does: count newlines, then add 1
        // This is because IndexBuilder uses line_idx + 1, and the last line_idx
        // is only populated when there's a trailing newline (giving an empty final line)
        let newline_count = content.iter().filter(|&&b| b == b'\n').count() as u32;
        let line_count = newline_count + 1;

        !meta.matches(mtime, size, hash, line_count)
    }

    pub fn invalidate(&self, source_file: &Path) -> std::io::Result<()> {
        let index_path = self.index_path(source_file);
        if index_path.exists() {
            fs::remove_file(index_path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_index_path() {
        let temp = TempDir::new().unwrap();
        let store = IndexStore::new(temp.path());

        let source = temp.path().join("src").join("main.rs");
        let index_path = store.index_path(&source);

        assert!(index_path.to_str().unwrap().contains(".linehash/indexes"));
        assert!(index_path.to_str().unwrap().ends_with(".lhidx"));
    }

    #[test]
    fn test_write_and_read() {
        let temp = TempDir::new().unwrap();
        let store = IndexStore::new(temp.path());

        let source = temp.path().join("test.txt");
        let content = b"hello\nworld\n";

        let index = store.write_index(&source, content, 12345).unwrap();

        assert_eq!(index.line_count, 3);
        assert!(index.trigram_count() > 0);

        let (read_index, meta) = store.read_index(&source).unwrap();
        assert_eq!(read_index.line_count(), 3);
        assert_eq!(meta.file_mtime, 12345);
    }

    #[test]
    fn test_stale_detection() {
        let temp = TempDir::new().unwrap();
        let store = IndexStore::new(temp.path());

        let source = temp.path().join("test.txt");
        let content = b"hello\nworld\n";

        // Write initial index
        store.write_index(&source, content, 12345).unwrap();

        // Same content should not be stale
        assert!(!store.is_stale(&source, content, 12345));

        // Different mtime should be stale
        assert!(store.is_stale(&source, content, 12346));

        // Different content should be stale
        assert!(store.is_stale(&source, b"different\n", 12345));
    }
}
