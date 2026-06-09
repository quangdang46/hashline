use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

use crate::document::{Document, FileMeta, FileStats, read_file_meta};
use crate::error::HashlineError;

/// A bounded document cache with LRU eviction, cache-hit/miss/invalidation
/// statistics, and a writeback path that lets mutation commands seed the
/// cache directly (avoiding an expensive disk re-read after every edit).
///
/// # LRU policy
///
/// When the cache is at capacity and a new entry needs to be inserted, the
/// *oldest* entry (by insertion time) is evicted. This is a simpler
/// approximation of LRU than a full linked-hash-map — adequate for the
/// MCP server's workload where files are typically opened in sequence and
/// only the most recent ones should remain cached.
///
/// # Thread safety
///
/// `SessionCache` is **not** `Send` or `Sync`. The MCP server is single
/// threaded (one request at a time on stdio), and the CLI is sequential,
/// so no synchronization is needed.
pub struct SessionCache {
    docs: HashMap<PathBuf, CacheEntry>,
    max_entries: usize,
    stats: CacheStats,
    no_cache: bool,
}

#[doc(hidden)]
pub struct CacheEntry {
    meta: FileMeta,
    doc: Document,
    stats: Option<FileStats>,
    created: Instant,
}

/// Aggregate cache statistics exposed via the MCP [`CacheStats`] payload.
#[derive(Default, Clone, Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub entries: usize,
    pub max_entries: usize,
}

impl SessionCache {
    /// Create a new cache with the given capacity.
    ///
    /// `max_entries` is the maximum number of cached files. When the cache
    /// is full, the oldest entry is evicted on the next insert. A value of
    /// `0` is treated as unbounded (no eviction).
    pub fn new(max_entries: usize) -> Self {
        Self {
            docs: HashMap::new(),
            max_entries,
            stats: CacheStats {
                max_entries,
                ..Default::default()
            },
            no_cache: false,
        }
    }

    /// Return or load the cache entry for `path`.
    ///
    /// If the file is already cached **and** its mtime (modification time)
    /// matches the file on disk, the cached document is returned and the
    /// hit counter is incremented.
    ///
    /// Otherwise the file is loaded from disk, the miss counter is
    /// incremented, and the entry is stored (potentially evicting an older
    /// entry if at capacity). The short-hash index is pre-populated so
    /// subsequent anchor resolutions skip the one-shot index build.
    pub fn get_or_load(&mut self, path: &Path) -> Result<&mut CacheEntry, HashlineError> {
        let meta = read_file_meta(path)?;
        let key = path.to_path_buf();

        // Fast path: return cached entry when mtime matches (and no_cache
        // is not active). Use a `bool` check to avoid holding a reference
        // across the subsequent mutable borrow of `self.docs`.
        if !self.no_cache {
            let is_hit = self.docs.get(&key).is_some_and(|entry| entry.meta == meta);
            if is_hit {
                self.stats.hits += 1;
                self.stats.entries = self.docs.len();
                return Ok(self
                    .docs
                    .get_mut(&key)
                    .expect("entry confirmed present above"));
            }
        }

        // Slow path: load from disk.
        let mut doc = Document::load(path)?;
        Document::build_index_cached(&mut doc); // Pre-populate cache
        self.stats.misses += 1;

        self.evict_one_if_full();
        let entry = CacheEntry {
            meta,
            doc,
            stats: None,
            created: Instant::now(),
        };
        self.docs.insert(key.clone(), entry);
        self.stats.entries = self.docs.len();

        self.docs.get_mut(&key).ok_or_else(|| {
            HashlineError::Io(std::io::Error::other(
                "session cache: entry vanished after insert",
            ))
        })
    }

    /// Remove the cache entry for `path`, if any.
    ///
    /// Used by mutation tools that modify a file so the next read triggers
    /// a fresh load. Increments the invalidation counter.
    pub fn invalidate(&mut self, path: &Path) {
        if self.docs.remove(path).is_some() {
            self.stats.invalidations += 1;
        }
        self.stats.entries = self.docs.len();
    }

    /// Insert (or replace) a document directly into the cache after a
    /// successful mutation, avoiding an expensive disk re-read.
    ///
    /// The caller should pass the *post-mutation* `Document` whose lines
    /// already reflect the edit. The cache stores it immediately and
    /// records the current file metadata so subsequent `get_or_load` calls
    /// hit the cache.
    pub fn after_mutation(&mut self, path: &Path, doc: Document) {
        let meta = match read_file_meta(path) {
            Ok(m) => m,
            Err(_) => {
                // If we cannot read metadata the file may have been deleted
                // or moved; invalidate and bail.
                self.invalidate(path);
                return;
            }
        };

        self.evict_one_if_full();
        self.docs.insert(
            path.to_path_buf(),
            CacheEntry {
                meta,
                doc,
                stats: None,
                created: Instant::now(),
            },
        );
        self.stats.entries = self.docs.len();
    }

    /// Return a reference to the current cache statistics.
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Remove all entries from the cache.
    pub fn clear(&mut self) {
        self.docs.clear();
        self.stats.entries = 0;
    }

    /// When `enabled` is `true`, `get_or_load` always loads from disk
    /// (bypassing the cache). Hits/misses are still tracked.
    pub fn set_no_cache(&mut self, enabled: bool) {
        self.no_cache = enabled;
    }

    // ---- internal helpers ----

    /// If the cache has reached its capacity, evict the oldest entry.
    fn evict_one_if_full(&mut self) {
        if self.max_entries == 0 {
            return;
        }
        if self.docs.len() < self.max_entries {
            return;
        }

        // Find the entry with the oldest `created` timestamp.
        let oldest_key = self
            .docs
            .iter()
            .min_by_key(|(_, entry)| entry.created)
            .map(|(key, _)| key.clone());

        if let Some(key) = oldest_key {
            self.docs.remove(&key);
        }
    }
}

impl CacheEntry {
    /// Return the file stats for this entry, computing them on first access
    /// and caching the result.
    pub fn stats(&mut self) -> &FileStats {
        self.stats.get_or_insert_with(|| self.doc.compute_stats())
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn doc_mut(&mut self) -> &mut Document {
        &mut self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_text(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn cache_hit_returns_same_doc_without_re_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\n");

        let mut cache = SessionCache::new(10);

        // First call: miss, load from disk
        let entry = cache.get_or_load(&path).unwrap();
        assert_eq!(entry.doc().lines[0].content.as_ref(), "alpha");
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // Second call: hit, no disk re-read
        let entry = cache.get_or_load(&path).unwrap();
        assert_eq!(entry.doc().lines[0].content.as_ref(), "alpha");
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn mtime_change_forces_re_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n");

        let mut cache = SessionCache::new(10);

        // Load once
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        // Change the file on disk
        write_text(&path, "beta\n");

        // Should miss again (mtime changed)
        let entry = cache.get_or_load(&path).unwrap();
        assert_eq!(entry.doc().lines[0].content.as_ref(), "beta");
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().hits, 0);
    }

    #[test]
    fn lru_eviction_when_over_max_entries() {
        let dir = TempDir::new().unwrap();
        let path_a = dir.path().join("a.txt");
        let path_b = dir.path().join("b.txt");
        let path_c = dir.path().join("c.txt");
        write_text(&path_a, "aaa\n");
        write_text(&path_b, "bbb\n");
        write_text(&path_c, "ccc\n");

        let mut cache = SessionCache::new(2); // Only 2 slots

        // Load a and b
        cache.get_or_load(&path_a).unwrap();
        assert_eq!(cache.stats().entries, 1);

        cache.get_or_load(&path_b).unwrap();
        assert_eq!(cache.stats().entries, 2);

        // Load c — should evict one (the oldest, a)
        cache.get_or_load(&path_c).unwrap();
        assert_eq!(cache.stats().entries, 2); // Still 2 entries

        // c and b should still be in cache; a should have been evicted.
        // Verify by checking hits/misses: a should be a miss now.
        let misses_before = cache.stats().misses;
        cache.get_or_load(&path_a).unwrap();
        assert_eq!(
            cache.stats().misses,
            misses_before + 1,
            "path_a should have been evicted (miss)"
        );
        assert_eq!(cache.stats().entries, 2);
    }

    #[test]
    fn after_mutation_updates_cache_without_disk_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\n");

        let mut cache = SessionCache::new(10);

        // Load initially
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().misses, 1);

        // Simulate a mutation: load a new document
        let mut new_doc = Document::load(&path).unwrap();
        let alpha_idx = new_doc
            .lines
            .iter()
            .position(|l| l.content.as_ref() == "alpha")
            .unwrap();
        use crate::mutation::replace_line;
        replace_line(&mut new_doc, alpha_idx, "ALPHA").unwrap();

        // Seed the cache with the post-mutation document
        cache.after_mutation(&path, new_doc);
        assert_eq!(cache.stats().entries, 1);

        // Read back — should be a hit with the mutated content
        let entry = cache.get_or_load(&path).unwrap();
        assert_eq!(entry.doc().lines[0].content.as_ref(), "ALPHA");
        assert_eq!(cache.stats().misses, 1); // No additional miss
        assert_eq!(cache.stats().hits, 1); // Hit from cache
    }

    #[test]
    fn no_cache_flag_bypasses_cache() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n");

        let mut cache = SessionCache::new(10);
        cache.set_no_cache(true);

        // First load: miss, loads from disk
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().misses, 1);

        // Second load with no_cache: still a miss, fresh load from disk
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().hits, 0);
    }

    #[test]
    fn invalidate_removes_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n");

        let mut cache = SessionCache::new(10);
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().entries, 1);

        cache.invalidate(&path);
        assert_eq!(cache.stats().invalidations, 1);
        assert_eq!(cache.stats().entries, 0);

        // Loading again should miss
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().misses, 2);
    }

    #[test]
    fn clear_removes_all_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n");

        let mut cache = SessionCache::new(10);
        cache.get_or_load(&path).unwrap();
        assert_eq!(cache.stats().entries, 1);

        cache.clear();
        assert_eq!(cache.stats().entries, 0);
    }
}
