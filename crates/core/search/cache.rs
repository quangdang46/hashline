#![allow(unused)]

//! In-memory cache for trigram indexes with incremental updates.
//!
//! This module provides an in-memory cache that stores trigram indexes
//! keyed by file path + content hash. It tracks file metadata (mtime, size)
//! to detect when indexes need rebuilding.
//!
//! # Cache Strategy
//!
//! - **Build-once**: Index is built once and cached in memory
//! - **Hash validation**: Content hash ensures index matches actual file
//! - **Automatic invalidation**: File modification invalidates cache entry
//! - **LRU eviction**: When max_capacity is reached, least-recently-used entries are evicted
//!
//! # Incremental Update
//!
//! For line-level changes (insert/delete/edit), the cache supports
//! targeted invalidation rather than full file re-indexing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::search::index::{IndexBuilder, compute_content_hash};
use crate::search::persist::IndexStore;
use crate::search::types::TrigramIndex;

/// Cache entry storing the index and its metadata.
struct CacheEntry {
    /// The cached trigram index.
    index: TrigramIndex,
    /// File mtime when index was built.
    mtime: u64,
    /// File size when index was built.
    size: u64,
    /// Content hash when index was built.
    content_hash: u64,
    /// Number of lines when index was built.
    line_count: u32,
    /// Last access time for LRU tracking.
    last_access: u64,
}

/// In-memory cache for trigram indexes with automatic invalidation.
///
/// # Type Parameters
///
/// - `P`: Persistence backend (e.g., `IndexStore` for disk persistence)
pub struct IndexCache {
    /// Backing persistence store for reading/writing indexes.
    store: IndexStore,
    /// In-memory cache: file path → cache entry.
    entries: HashMap<PathBuf, CacheEntry>,
    /// Maximum number of entries to cache (0 = unlimited).
    max_capacity: usize,
    /// Access counter for LRU eviction.
    access_counter: u64,
    /// Whether to use persistent storage.
    use_persistence: bool,
}

impl IndexCache {
    /// Create a new index cache.
    ///
    /// # Arguments
    ///
    /// * `root` - Root directory for the cache (used for persistent storage path)
    /// * `max_capacity` - Maximum number of entries (0 = unlimited)
    /// * `use_persistence` - Whether to persist indexes to disk
    pub fn new(root: impl AsRef<Path>, max_capacity: usize, use_persistence: bool) -> Self {
        Self {
            store: IndexStore::new(root),
            entries: HashMap::new(),
            max_capacity,
            access_counter: 0,
            use_persistence,
        }
    }

    /// Create a cache with default settings (no persistence, unlimited capacity).
    pub fn default() -> Self {
        Self::new(".", 0, false)
    }

    /// Get an index for the given file, building if necessary.
    ///
    /// Returns the cached index if valid, or builds a new one.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file
    /// * `content` - File content as bytes
    /// * `mtime` - File modification time (seconds since epoch)
    ///
    /// # Returns
    ///
    /// Returns a reference to the cached `TrigramIndex`.
    pub fn get_index(
        &mut self,
        path: &Path,
        content: &[u8],
        mtime: u64,
    ) -> std::io::Result<&TrigramIndex> {
        let size = content.len() as u64;
        let content_hash = compute_content_hash(content);
        let line_count = Self::count_lines(content);

        let path_buf = path.to_path_buf();
        let needs_rebuild = match self.entries.get(&path_buf) {
            Some(entry) => {
                entry.mtime != mtime
                    || entry.size != size
                    || entry.content_hash != content_hash
                    || entry.line_count != line_count
            }
            None => true,
        };

        if !needs_rebuild {
            self.access_counter += 1;
            self.entries.get_mut(&path_buf).unwrap().last_access = self.access_counter;
            return Ok(&self.entries.get(&path_buf).unwrap().index);
        }

        if self.use_persistence {
            if let Ok((_mmap_index, meta)) = self.store.read_index(&path_buf) {
                if meta.file_mtime == mtime
                    && meta.file_size == size
                    && meta.content_hash == content_hash
                    && meta.line_count == line_count
                {
                    let mut builder = IndexBuilder::new();
                    let line_count_usize = meta.line_count as usize;
                    for (idx, line) in content.split(|&b| b == b'\n').enumerate() {
                        if idx < line_count_usize {
                            builder.add_line(idx, line);
                        }
                    }
                    let index = builder.build();

                    self.access_counter += 1;
                    self.evict_if_needed()?;

                    let entry = CacheEntry {
                        index,
                        mtime,
                        size,
                        content_hash,
                        line_count,
                        last_access: self.access_counter,
                    };
                    self.entries.insert(path_buf.clone(), entry);
                    return Ok(&self.entries.get(&path_buf).unwrap().index);
                }
            }
        }

        let mut builder = IndexBuilder::new();
        for (idx, line) in content.split(|&b| b == b'\n').enumerate() {
            builder.add_line(idx, line);
        }
        let index = builder.build();

        self.access_counter += 1;
        self.evict_if_needed()?;

        let entry = CacheEntry {
            index,
            mtime,
            size,
            content_hash,
            line_count,
            last_access: self.access_counter,
        };
        self.entries.insert(path_buf, entry);

        if self.use_persistence {
            let _ = self.store.write_index(path, content, mtime);
        }

        Ok(&self.entries.get(path).unwrap().index)
    }

    /// Get an index with a provided TrigramIndex (for cases where caller has already built one).
    pub fn put_index(
        &mut self,
        path: &Path,
        index: TrigramIndex,
        mtime: u64,
        size: u64,
        content_hash: u64,
        line_count: u32,
    ) -> std::io::Result<()> {
        self.access_counter += 1;
        self.evict_if_needed()?;

        let entry = CacheEntry {
            index,
            mtime,
            size,
            content_hash,
            line_count,
            last_access: self.access_counter,
        };
        self.entries.insert(path.to_path_buf(), entry);
        Ok(())
    }

    /// Invalidate the cache entry for a file.
    ///
    /// Use this when the file has been modified and the cache should be ignored.
    pub fn invalidate(&mut self, path: &Path) -> std::io::Result<()> {
        self.entries.remove(path);
        if self.use_persistence {
            self.store.invalidate(path)?;
        }
        Ok(())
    }

    /// Invalidate all cache entries.
    pub fn invalidate_all(&mut self) -> std::io::Result<()> {
        self.entries.clear();
        Ok(())
    }

    /// Check if an entry exists and is valid for the given file stats.
    pub fn is_valid(
        &self,
        path: &Path,
        mtime: u64,
        size: u64,
        content_hash: u64,
        line_count: u32,
    ) -> bool {
        if let Some(entry) = self.entries.get(path) {
            entry.mtime == mtime
                && entry.size == size
                && entry.content_hash == content_hash
                && entry.line_count == line_count
        } else {
            false
        }
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let mut total_trigrams = 0;
        let mut total_postings = 0;
        for entry in self.entries.values() {
            total_trigrams += entry.index.trigram_count();
            total_postings += entry.index.posting_count();
        }
        CacheStats {
            entry_count: self.entries.len(),
            total_trigrams,
            total_postings,
        }
    }

    /// Count lines in content (same algorithm as IndexBuilder).
    fn count_lines(content: &[u8]) -> u32 {
        let newline_count = content.iter().filter(|&&b| b == b'\n').count() as u32;
        newline_count + 1
    }

    /// Evict least-recently-used entry if over capacity.
    fn evict_if_needed(&mut self) -> std::io::Result<()> {
        if self.max_capacity == 0 {
            return Ok(());
        }

        while self.entries.len() >= self.max_capacity {
            // Find LRU entry
            let lru_path = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(p, _)| p.clone());

            if let Some(path) = lru_path {
                self.entries.remove(&path);
            } else {
                break;
            }
        }
        Ok(())
    }
}

impl Default for IndexCache {
    fn default() -> Self {
        Self::default()
    }
}

/// Statistics about the index cache.
#[derive(Clone, Debug, Default)]
pub struct CacheStats {
    /// Number of cached entries.
    pub entry_count: usize,
    /// Total number of trigrams across all entries.
    pub total_trigrams: usize,
    /// Total number of postings across all entries.
    pub total_postings: usize,
}

/// Thread-safe wrapper around IndexCache for use in CLI/MCP contexts.
///
/// This wrapper uses a RwLock to allow concurrent reads while maintaining
/// exclusive write access for cache updates.
#[derive(Clone)]
pub struct SharedIndexCache {
    inner: Arc<RwLock<IndexCache>>,
}

impl SharedIndexCache {
    /// Create a new shared cache.
    pub fn new(root: impl AsRef<Path>, max_capacity: usize, use_persistence: bool) -> Self {
        Self {
            inner: Arc::new(RwLock::new(IndexCache::new(
                root,
                max_capacity,
                use_persistence,
            ))),
        }
    }

    /// Create with default settings.
    pub fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(IndexCache::default())),
        }
    }

    /// Get an index for the given file.
    pub fn get_index(
        &self,
        path: &Path,
        content: &[u8],
        mtime: u64,
    ) -> std::io::Result<TrigramIndex> {
        let mut cache = self.inner.write().unwrap();
        // We need to return a owned index since the borrow checker
        // can't ensure the lock is held while the caller uses the index.
        // Clone the index since TrigramIndex is Clone.
        let index = cache.get_index(path, content, mtime)?.clone();
        Ok(index)
    }

    /// Put an index into the cache.
    pub fn put_index(
        &self,
        path: &Path,
        index: TrigramIndex,
        mtime: u64,
        size: u64,
        content_hash: u64,
        line_count: u32,
    ) -> std::io::Result<()> {
        let mut cache = self.inner.write().unwrap();
        cache.put_index(path, index, mtime, size, content_hash, line_count)
    }

    /// Invalidate the cache for a file.
    pub fn invalidate(&self, path: &Path) -> std::io::Result<()> {
        let mut cache = self.inner.write().unwrap();
        cache.invalidate(path)
    }

    /// Invalidate all entries.
    pub fn invalidate_all(&self) -> std::io::Result<()> {
        let mut cache = self.inner.write().unwrap();
        cache.invalidate_all()
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let cache = self.inner.read().unwrap();
        cache.stats()
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        let cache = self.inner.read().unwrap();
        cache.len()
    }

    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        let cache = self.inner.read().unwrap();
        cache.is_empty()
    }
}

impl Default for SharedIndexCache {
    fn default() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cache_basic() {
        let temp = TempDir::new().unwrap();
        let mut cache = IndexCache::new(temp.path(), 2, false);

        let content1 = b"hello\nworld\n";
        let content2 = b"foo\nbar\nbaz\n";

        let idx1 = cache
            .get_index(&temp.path().join("file1.txt"), content1, 1000)
            .unwrap();
        assert_eq!(idx1.line_count, 3);

        let idx2 = cache
            .get_index(&temp.path().join("file2.txt"), content2, 1000)
            .unwrap();
        assert_eq!(idx2.line_count, 4);

        assert_eq!(cache.len(), 2);

        let content3 = b"alpha\nbeta\n";
        let _idx3 = cache
            .get_index(&temp.path().join("file3.txt"), content3, 1000)
            .unwrap();

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_cache_invalidation() {
        let temp = TempDir::new().unwrap();
        let mut cache = IndexCache::new(temp.path(), 0, false);

        let content = b"hello\nworld\n";
        let path = temp.path().join("file.txt");

        cache.get_index(&path, content, 1000).unwrap();
        assert_eq!(cache.len(), 1);

        cache.invalidate(&path).unwrap();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_content_hash_validation() {
        let temp = TempDir::new().unwrap();
        let mut cache = IndexCache::new(temp.path(), 0, false);

        let path = temp.path().join("file.txt");

        // First content
        let content1 = b"hello\nworld\n";
        cache.get_index(&path, content1, 1000).unwrap();

        // Same mtime but different content should rebuild
        let content2 = b"different\ncontent\n";
        let idx2 = cache.get_index(&path, content2, 1000).unwrap();
        assert_eq!(idx2.line_count, 3); // content2 has 3 lines (including trailing empty)

        // Different mtime with same content should rebuild
        let content1_copy = b"hello\nworld\n";
        let idx3 = cache.get_index(&path, content1_copy, 1001).unwrap();
        assert_eq!(idx3.line_count, 3); // content1_copy has 3 lines (including trailing empty)
    }

    #[test]
    fn test_cache_stats() {
        let temp = TempDir::new().unwrap();
        let mut cache = IndexCache::new(temp.path(), 0, false);

        let content = b"hello\nworld\n";
        cache
            .get_index(&temp.path().join("file1.txt"), content, 1000)
            .unwrap();
        cache
            .get_index(&temp.path().join("file2.txt"), content, 1000)
            .unwrap();

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 2);
        assert!(stats.total_trigrams > 0);
    }

    #[test]
    fn test_shared_cache() {
        let cache = SharedIndexCache::default();

        let temp = TempDir::new().unwrap();
        let content = b"hello\nworld\n";

        let idx = cache
            .get_index(&temp.path().join("file.txt"), content, 1000)
            .unwrap();
        assert_eq!(idx.line_count, 3); // content has 3 lines (including trailing empty)

        cache.invalidate(&temp.path().join("file.txt")).unwrap();
        assert!(cache.is_empty());
    }
}
