use lru::LruCache;
use std::sync::Arc;
use std::time::Duration;

use super::token::TokenIndex;
use super::trigram::TrigramIndex;

/// Cache entry wrapping the token and trigram indexes.
#[derive(Clone)]
pub struct IndexCacheEntry {
    pub token_index: Arc<TokenIndex>,
    pub trigram_index: Arc<TrigramIndex>,
    pub mtime: u64,
    pub size: u64,
    pub content_hash: u64,
}

/// LRU cache for file indexes, validated against file metadata.
pub struct IndexCache {
    inner: LruCache<String, IndexCacheEntry>,
    ttl: Duration,
}

impl IndexCache {
    /// Create a new cache with the given maximum capacity and TTL.
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            inner: LruCache::new(std::num::NonZeroUsize::new(capacity).unwrap()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Look up an entry, checking metadata validity.
    pub fn get(
        &mut self,
        path: &str,
        mtime: u64,
        size: u64,
        content_hash: u64,
    ) -> Option<IndexCacheEntry> {
        let entry = self.inner.get(path)?;

        if entry.mtime != mtime || entry.size != size || entry.content_hash != content_hash {
            self.inner.pop(&path.to_string());
            return None;
        }

        Some(entry.clone())
    }

    /// Insert an entry into the cache.
    pub fn insert(&mut self, path: String, entry: IndexCacheEntry) {
        self.inner.put(path, entry);
    }

    /// Invalidate a specific path.
    pub fn invalidate(&mut self, path: &str) {
        self.inner.pop(&path.to_string());
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Return the number of cached entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

/// Shared cache wrapper for use behind a lock.
pub type SharedIndexCache = parking_lot::RwLock<IndexCache>;

impl Default for IndexCache {
    fn default() -> Self {
        Self::new(128, 300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss_on_mismatch() {
        let mut cache = IndexCache::new(10, 60);
        // No entry exists yet
        assert!(cache.get("foo.txt", 0, 0, 0).is_none());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = IndexCache::new(10, 60);
        let entry = IndexCacheEntry {
            token_index: Arc::new(TokenIndex::build("", &[0])),
            trigram_index: Arc::new(TrigramIndex::build_from_content("", &[0])),
            mtime: 100,
            size: 50,
            content_hash: 0xDEAD,
        };
        cache.insert("foo.txt".to_string(), entry.clone());
        let found = cache.get("foo.txt", 100, 50, 0xDEAD);
        assert!(found.is_some());
    }

    #[test]
    fn test_cache_invalidate() {
        let mut cache = IndexCache::new(10, 60);
        let entry = IndexCacheEntry {
            token_index: Arc::new(TokenIndex::build("", &[0])),
            trigram_index: Arc::new(TrigramIndex::build_from_content("", &[0])),
            mtime: 100,
            size: 50,
            content_hash: 0xDEAD,
        };
        cache.insert("foo.txt".to_string(), entry);
        cache.invalidate("foo.txt");
        assert!(cache.get("foo.txt", 100, 50, 0xDEAD).is_none());
    }
}
