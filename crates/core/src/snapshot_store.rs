//! Per-call snapshot store for hashline section tags.
//!
//! Mirrors the TypeScript `SnapshotStore` from the original hashline project at
//! `/packages/hashline/src/snapshots.ts`. Provides a trait and an in-memory
//! implementation backed by `HashMap` + `Vec` with simple first-in eviction
//! (no external LRU crate).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// One full-file version observed at a point in time. The tag the model sees is
/// [`Snapshot::hash`]; recovery replays edits against [`Snapshot::text`].
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Canonical path this version belongs to.
    pub path: String,
    /// Full normalized (LF, no BOM) file text as observed.
    pub text: String,
    /// Content-derived tag for `text` (see [`compute_file_hash`]).
    pub hash: String,
    /// Timestamp (ms since epoch) the version was recorded.
    pub recorded_at: u64,
    /// 1-indexed file lines a producer (read/search) actually *displayed* under
    /// this tag. `None` means "no provenance recorded" — the patcher then skips
    /// the seen-line check and applies as before.
    pub seen_lines: Option<HashSet<usize>>,
}

/// Storage seam for full-file version snapshots. The patcher calls [`head`]
/// for the latest version of a path and [`by_hash`] when it needs the specific
/// historical version a section's stale tag names.
pub trait SnapshotStore: Send + Sync {
    /// Most-recently recorded version for `path`, or `None` if none.
    fn head(&self, path: &str) -> Option<Snapshot>;

    /// Recorded version for `path` whose tag equals `hash`, or `None`.
    fn by_hash(&self, path: &str, hash: &str) -> Option<Snapshot>;

    /// Record the full normalized text of `path` and return its content tag.
    /// `seen_lines` (optional) are the 1-indexed lines the producer displayed;
    /// they merge into [`Snapshot::seen_lines`] across reads of identical text.
    fn record(&mut self, path: &str, full_text: &str, seen_lines: Option<&[usize]>) -> String;

    /// Merge `lines` into the [`Snapshot::seen_lines`] of the version whose tag
    /// equals `hash`. No-op when no such version is retained (the content aged
    /// out or was overwritten).
    fn record_seen_lines(&mut self, path: &str, hash: &str, lines: &[usize]);

    /// Drop the version history for a single path.
    fn invalidate(&mut self, path: &str);

    /// Drop every version history.
    fn clear(&mut self);
}

// ---------------------------------------------------------------------------
// Default limits (mirrors snapshots.ts)
// ---------------------------------------------------------------------------

const DEFAULT_MAX_PATHS: usize = 30;
const DEFAULT_MAX_VERSIONS_PER_PATH: usize = 4;
const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current timestamp as milliseconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Union `lines` into `snapshot.seen_lines`, lazily creating the set.
fn merge_seen_lines(snapshot: &mut Snapshot, lines: Option<&[usize]>) {
    let Some(lines) = lines else { return };
    if lines.is_empty() {
        return;
    }
    let set = snapshot.seen_lines.get_or_insert_with(HashSet::new);
    for &line in lines {
        set.insert(line);
    }
}

// ---------------------------------------------------------------------------
// InMemorySnapshotStore
// ---------------------------------------------------------------------------

/// In-memory [`SnapshotStore`] backed by a `HashMap` of path histories.
///
/// Per-path history is a short ring of full-file versions (oldest dropped first);
/// per-session path tracking evicts cold paths when limits are exceeded.
///
/// Recording byte-identical content again refreshes recency and reuses the
/// existing tag (read fusion); recording new content prepends a fresh version
/// onto the front of the path history.
pub struct InMemorySnapshotStore {
    /// Map from canonical path to its version history (most recent first).
    versions: HashMap<String, Vec<Snapshot>>,
    /// Path access order for LRU eviction. Front = oldest, back = newest.
    access_order: VecDeque<String>,
    /// Maximum number of distinct paths (default 30).
    max_paths: usize,
    /// Maximum full-file versions retained per path (default 4).
    max_versions_per_path: usize,
    /// Global ceiling on retained snapshot text across all paths (default 64 MiB).
    max_total_bytes: usize,
    /// Sum of `text.len()` across all retained snapshots.
    total_bytes: usize,
}

impl InMemorySnapshotStore {
    /// Create a new store with default limits.
    pub fn new() -> Self {
        Self {
            versions: HashMap::new(),
            access_order: VecDeque::new(),
            max_paths: DEFAULT_MAX_PATHS,
            max_versions_per_path: DEFAULT_MAX_VERSIONS_PER_PATH,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            total_bytes: 0,
        }
    }

    /// Create a store with custom limits.
    pub fn with_options(
        max_paths: usize,
        max_versions_per_path: usize,
        max_total_bytes: usize,
    ) -> Self {
        Self {
            versions: HashMap::new(),
            access_order: VecDeque::new(),
            max_paths,
            max_versions_per_path,
            max_total_bytes,
            total_bytes: 0,
        }
    }

    /// Evict the least-recently-used path (from the front of `access_order`).
    fn evict_one(&mut self) {
        let Some(path) = self.access_order.pop_front() else {
            return;
        };
        if let Some(history) = self.versions.remove(&path) {
            for s in &history {
                self.total_bytes = self.total_bytes.saturating_sub(s.text.len());
            }
        }
    }

    /// Ensure limits are satisfied. Evicts oldest paths until both
    /// `max_paths` and `max_total_bytes` are respected.
    fn enforce_limits(&mut self) {
        while self.access_order.len() > self.max_paths {
            self.evict_one();
        }
        while self.total_bytes > self.max_total_bytes && !self.access_order.is_empty() {
            self.evict_one();
        }
    }

    /// The number of distinct paths currently tracked.
    #[allow(dead_code)]
    pub fn path_count(&self) -> usize {
        self.versions.len()
    }

    /// The total byte count of all retained snapshot text.
    #[allow(dead_code)]
    pub fn total_byte_count(&self) -> usize {
        self.total_bytes
    }
}

impl Default for InMemorySnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotStore for InMemorySnapshotStore {
    fn head(&self, path: &str) -> Option<Snapshot> {
        self.versions.get(path)?.first().cloned()
    }

    fn by_hash(&self, path: &str, hash: &str) -> Option<Snapshot> {
        self.versions
            .get(path)?
            .iter()
            .find(|s| s.hash == hash)
            .cloned()
    }

    fn record(&mut self, path: &str, full_text: &str, seen_lines: Option<&[usize]>) -> String {
        let hash = crate::hash::compute_file_hash(full_text);
        let text_len = full_text.len();

        // Scope the mutable borrow of self.versions so we can touch
        // self.access_order afterwards.
        {
            let history = self
                .versions
                .entry(path.to_owned())
                .or_default();

            // Read fusion: same content observed again.
            if let Some(pos) = history.iter().position(|s| s.hash == hash) {
                let snapshot = &mut history[pos];
                snapshot.recorded_at = now();
                merge_seen_lines(snapshot, seen_lines);

                // Promote to front (most recent).
                if pos != 0 {
                    let s = history.remove(pos);
                    history.insert(0, s);
                }
            } else {
                // New snapshot.
                let snapshot = Snapshot {
                    path: path.to_owned(),
                    text: full_text.to_owned(),
                    hash: hash.clone(),
                    recorded_at: now(),
                    seen_lines: seen_lines.map(|lines| lines.iter().copied().collect()),
                };

                history.insert(0, snapshot);
                self.total_bytes += text_len;

                // Trim per-path history to max_versions_per_path (oldest
                // dropped first).
                while history.len() > self.max_versions_per_path {
                    let removed = history.pop().unwrap();
                    self.total_bytes = self.total_bytes.saturating_sub(removed.text.len());
                }
            }
        }

        // Bring path to the back of the access order (most recently used).
        if let Some(pos) = self.access_order.iter().position(|p| p == path) {
            self.access_order.remove(pos);
        }
        self.access_order.push_back(path.to_owned());

        // Evict cold paths if limits are exceeded.
        self.enforce_limits();

        hash
    }

    fn record_seen_lines(&mut self, path: &str, hash: &str, lines: &[usize]) {
        if lines.is_empty() {
            return;
        }
        let Some(history) = self.versions.get_mut(path) else {
            return;
        };
        let Some(snapshot) = history.iter_mut().find(|s| s.hash == hash) else {
            return;
        };
        let set = snapshot.seen_lines.get_or_insert_with(HashSet::new);
        for &line in lines {
            set.insert(line);
        }
    }

    fn invalidate(&mut self, path: &str) {
        if let Some(history) = self.versions.remove(path) {
            for s in &history {
                self.total_bytes = self.total_bytes.saturating_sub(s.text.len());
            }
        }
        self.access_order.retain(|p| p != path);
    }

    fn clear(&mut self) {
        self.versions.clear();
        self.access_order.clear();
        self.total_bytes = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_head_none_when_empty() {
        let store = InMemorySnapshotStore::new();
        assert!(store.head("/dev/null").is_none());
    }

    #[test]
    fn test_record_and_head() {
        let mut store = InMemorySnapshotStore::new();
        let tag = store.record("/tmp/a.txt", "hello\nworld\n", None);

        let snap = store.head("/tmp/a.txt").unwrap();
        assert_eq!(snap.hash, tag);
        assert_eq!(snap.text, "hello\nworld\n");
        assert!(snap.seen_lines.is_none());
    }

    #[test]
    fn test_by_hash_finds_recorded_snapshot() {
        let mut store = InMemorySnapshotStore::new();
        let tag = store.record("/tmp/b.txt", "content", None);

        let found = store.by_hash("/tmp/b.txt", &tag).unwrap();
        assert_eq!(found.text, "content");

        // Wrong hash returns None
        assert!(store.by_hash("/tmp/b.txt", "DEAD").is_none());
    }

    #[test]
    fn test_read_fusion_reuses_same_hash() {
        let mut store = InMemorySnapshotStore::new();

        let tag1 = store.record("/tmp/fuse.txt", "same data", None);
        let tag2 = store.record("/tmp/fuse.txt", "same data", None);

        assert_eq!(tag1, tag2, "read fusion must reuse the same tag");

        // Only one snapshot retained.
        let history = store.versions.get("/tmp/fuse.txt").unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_read_fusion_updates_seen_lines() {
        let mut store = InMemorySnapshotStore::new();

        let tag = store.record("/tmp/fuse.txt", "data", Some(&[1, 2]));
        let tag2 = store.record("/tmp/fuse.txt", "data", Some(&[3, 4]));

        assert_eq!(tag, tag2);
        let snap = store.head("/tmp/fuse.txt").unwrap();
        let seen = snap.seen_lines.as_ref().unwrap();
        assert!(seen.contains(&1));
        assert!(seen.contains(&2));
        assert!(seen.contains(&3));
        assert!(seen.contains(&4));
    }

    #[test]
    fn test_new_content_creates_new_snapshot() {
        let mut store = InMemorySnapshotStore::new();

        let tag1 = store.record("/tmp/vers.txt", "version 1", None);
        let tag2 = store.record("/tmp/vers.txt", "version 2", None);

        assert_ne!(tag1, tag2);

        // Head returns the latest.
        assert_eq!(store.head("/tmp/vers.txt").unwrap().hash, tag2);

        // Both versions still accessible by hash.
        assert!(store.by_hash("/tmp/vers.txt", &tag1).is_some());
        assert!(store.by_hash("/tmp/vers.txt", &tag2).is_some());
    }

    #[test]
    fn test_history_truncated_at_max_versions() {
        let mut store = InMemorySnapshotStore::with_options(30, 2, 1024 * 1024);

        let t1 = store.record("/tmp/trunc.txt", "v1", None);
        let t2 = store.record("/tmp/trunc.txt", "v2", None);
        let t3 = store.record("/tmp/trunc.txt", "v3", None);

        // Only the two most recent versions are kept.
        assert!(store.by_hash("/tmp/trunc.txt", &t1).is_none());
        assert!(store.by_hash("/tmp/trunc.txt", &t2).is_some());
        assert!(store.by_hash("/tmp/trunc.txt", &t3).is_some());
    }

    #[test]
    fn test_invalidate_removes_path() {
        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/gone.txt", "content", None);
        assert!(store.head("/tmp/gone.txt").is_some());

        store.invalidate("/tmp/gone.txt");
        assert!(store.head("/tmp/gone.txt").is_none());
    }

    #[test]
    fn test_clear_removes_everything() {
        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/a.txt", "aaa", None);
        store.record("/tmp/b.txt", "bbb", None);
        assert_eq!(store.path_count(), 2);

        store.clear();
        assert_eq!(store.path_count(), 0);
        assert!(store.head("/tmp/a.txt").is_none());
    }

    #[test]
    fn test_max_paths_eviction() {
        let mut store = InMemorySnapshotStore::with_options(2, 4, 1024 * 1024);

        let t1 = store.record("/tmp/p1.txt", "path 1", None);
        let t2 = store.record("/tmp/p2.txt", "path 2", None);

        // Both fit.
        assert!(store.by_hash("/tmp/p1.txt", &t1).is_some());
        assert!(store.by_hash("/tmp/p2.txt", &t2).is_some());

        // Adding a third evicts the oldest (p1).
        let t3 = store.record("/tmp/p3.txt", "path 3", None);
        assert!(store.by_hash("/tmp/p1.txt", &t1).is_none());
        assert!(store.by_hash("/tmp/p2.txt", &t2).is_some());
        assert!(store.by_hash("/tmp/p3.txt", &t3).is_some());
    }

    #[test]
    fn test_record_seen_lines() {
        let mut store = InMemorySnapshotStore::new();
        let tag = store.record("/tmp/sl.txt", "line1\nline2\nline3\n", None);

        store.record_seen_lines("/tmp/sl.txt", &tag, &[1, 3]);

        let snap = store.head("/tmp/sl.txt").unwrap();
        let seen = snap.seen_lines.as_ref().unwrap();
        assert!(seen.contains(&1));
        assert!(!seen.contains(&2));
        assert!(seen.contains(&3));
    }

    #[test]
    fn test_record_seen_lines_noop_for_stale_hash() {
        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/nop.txt", "content", None);
        // No crash, just a no-op.
        store.record_seen_lines("/tmp/nop.txt", "STALE", &[42]);
    }

    #[test]
    fn test_by_hash_on_missing_path_returns_none() {
        let store = InMemorySnapshotStore::new();
        assert!(store.by_hash("/tmp/nope.txt", "ABCD").is_none());
    }

    #[test]
    fn test_total_bytes_tracked() {
        let mut store = InMemorySnapshotStore::with_options(30, 4, 1024 * 1024);

        store.record("/tmp/bytes.txt", "hello", None);
        assert_eq!(store.total_byte_count(), 5);

        store.record("/tmp/bytes.txt", "hello world", None);
        assert_eq!(store.total_byte_count(), 5 + 11);

        // Invalidate reduces the count.
        store.invalidate("/tmp/bytes.txt");
        assert_eq!(store.total_byte_count(), 0);
    }

    #[test]
    fn test_total_bytes_eviction() {
        // Tiny byte limit: only 10 bytes of text total.
        let mut store = InMemorySnapshotStore::with_options(30, 4, 10);

        store.record("/tmp/small.txt", "aaaa", None); // 4 bytes
        assert_eq!(store.total_byte_count(), 4);

        store.record("/tmp/large.txt", "bbbbbbbbbb", None); // 10 bytes — total = 14 > 10
        // small.txt should be evicted.
        assert!(store.head("/tmp/small.txt").is_none());
        assert_eq!(store.total_byte_count(), 10);
    }
}
