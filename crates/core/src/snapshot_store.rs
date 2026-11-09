//! Per-call snapshot store for hashline section tags.
//!
//! Mirrors the TypeScript `SnapshotStore` from the original hashline project at
//! `/packages/hashline/src/snapshots.ts`. Provides a trait and an in-memory
//! implementation backed by `HashMap` + `Vec` with simple first-in eviction
//! (no external LRU crate).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

/// A [`SnapshotStore`] that records nothing and returns nothing.
///
/// Use this when you want hashline to operate without any snapshot caching
/// whatsoever — no memory overhead, no state tracking. Every patch is
/// applied blind (no stale-anchor detection against a prior snapshot;
/// anchors are still validated against the file on disk).
///
/// This is the equivalent of `enable_snapshots: false` in
/// [`HashlineConfig`](crate::config::HashlineConfig).
pub struct NoopSnapshotStore;

impl NoopSnapshotStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopSnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotStore for NoopSnapshotStore {
    fn head(&self, _path: &str) -> Option<Snapshot> {
        None
    }

    fn by_hash(&self, _path: &str, _hash: &str) -> Option<Snapshot> {
        None
    }

    fn by_content(&self, _path: &str, _full_text: &str) -> Option<Snapshot> {
        None
    }

    fn record(&mut self, _path: &str, full_text: &str, _seen_lines: Option<&[usize]>) -> String {
        let normalized = normalize_contract(full_text);
        crate::hash::compute_file_hash(&normalized)
    }

    fn record_seen_lines(&mut self, _path: &str, _hash: &str, _lines: &[usize]) {}

    fn invalidate(&mut self, _path: &str) {}

    fn clear(&mut self) {}
}
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

    /// Recorded version for `path` whose text equals `full_text`, or `None`.
    /// The patcher uses it on the no-drift path to attach seen-line provenance
    /// to the exact text the model read, and to disambiguate two texts that
    /// collide on a 16-bit tag.
    ///
    /// `full_text` is normalized to the store contract (LF, no BOM) before
    /// comparison, so callers may pass raw file bytes.
    fn by_content(&self, path: &str, full_text: &str) -> Option<Snapshot>;

    /// All retained versions for `path`, most-recently-recorded first.
    ///
    /// The patcher uses this to attempt stale-anchor recovery against each
    /// snapshot a producer may have anchored to — not just the head. The
    /// default returns an empty list, which is correct for stores that keep
    /// no history; consumers that want multi-version recovery should
    /// override it.
    fn versions(&self, _path: &str) -> Vec<Snapshot> {
        Vec::new()
    }

    /// Record the full text of `path` and return its content tag.
    /// `full_text` is normalized to the store contract (LF, no BOM) before
    /// hashing and storing — callers may pass raw file bytes.
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

/// Lexically canonicalize a path key so aliases map to one store entry.
///
/// Collapses `\` to `/`, removes `.` segments and empty segments, and
/// resolves `..` by popping the previous segment. This is deliberately
/// *lexical* — it never touches the filesystem, so it works for paths that
/// do not exist yet and cannot fail. Two spellings of the same file
/// (`./a.txt`, `dir/../a.txt`, `a\b.txt` vs `a/b.txt`) share one history.
fn canonical_key(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let absolute = unified.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    if absolute && !joined.is_empty() {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Normalize text to the store's contract: LF line endings, no BOM.
///
/// [`Snapshot::text`] and the recorded hash always describe this form, so
/// raw file bytes (which may be CRLF) are normalized on the way in. Returns
/// a borrow when the text already satisfies the contract.
fn normalize_contract(text: &str) -> Cow<'_, str> {
    if text.contains('\r') || text.starts_with('\u{FEFF}') {
        let lf = crate::normalize::normalize_to_lf(text);
        Cow::Owned(crate::normalize::strip_bom(&lf).text)
    } else {
        Cow::Borrowed(text)
    }
}

/// Bytes attributable to one snapshot: the text plus the capacity of its
/// `seen_lines` set. The seen-lines set is bounded by the file's line count
/// but was previously untracked, letting a consumer reading many large
/// files grow memory without limit despite `max_total_bytes`.
fn snapshot_bytes(s: &Snapshot) -> usize {
    s.text.len()
        + s.seen_lines
            .as_ref()
            .map_or(0, |set| set.capacity() * std::mem::size_of::<usize>())
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

/// Current timestamp as milliseconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
    /// Global ceiling on retained snapshot bytes across all paths
    /// (default 64 MiB). Counts snapshot text *and* `seen_lines` set
    /// capacity — see [`snapshot_bytes`].
    max_total_bytes: usize,
    /// Sum of [`snapshot_bytes`] across all retained snapshots.
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
                self.total_bytes = self.total_bytes.saturating_sub(snapshot_bytes(s));
            }
        }
    }

    /// Ensure limits are satisfied. Evicts oldest paths until both
    /// `max_paths` and `max_total_bytes` are respected.
    ///
    /// The byte-limit loop always keeps the most-recently-used path: a
    /// single snapshot larger than `max_total_bytes` (a huge file, or a
    /// huge `seen_lines` set) must not evict *itself* and leave the store
    /// empty — it is retained, exceeding the cap, until a newer record
    /// for another path pushes it out via the normal LRU order.
    fn enforce_limits(&mut self) {
        while self.access_order.len() > self.max_paths {
            self.evict_one();
        }
        while self.total_bytes > self.max_total_bytes && self.access_order.len() > 1 {
            self.evict_one();
        }
    }

    /// The number of distinct paths currently tracked.
    #[allow(dead_code)]
    pub fn path_count(&self) -> usize {
        self.versions.len()
    }

    /// The total byte count of all retained snapshots, including
    /// `seen_lines` set capacity (see [`snapshot_bytes`]).
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
        self.versions.get(&canonical_key(path))?.first().cloned()
    }

    fn by_hash(&self, path: &str, hash: &str) -> Option<Snapshot> {
        // History is stored most-recent-first, so the first version whose tag
        // matches IS the most recent. Resolving a 16-bit tag collision to the
        // most-recent version (rather than rejecting) is the oh-my-pi 16.3.3
        // behavior; recovery still requires the anchors/context to remap
        // unambiguously, so a stale collision fails closed downstream.
        self.versions
            .get(&canonical_key(path))?
            .iter()
            .find(|s| s.hash == hash)
            .cloned()
    }

    fn by_content(&self, path: &str, full_text: &str) -> Option<Snapshot> {
        let normalized = normalize_contract(full_text);
        self.versions
            .get(&canonical_key(path))?
            .iter()
            .find(|s| s.text.as_str() == normalized.as_ref())
            .cloned()
    }

    fn versions(&self, path: &str) -> Vec<Snapshot> {
        self.versions
            .get(&canonical_key(path))
            .cloned()
            .unwrap_or_default()
    }

    fn record(&mut self, path: &str, full_text: &str, seen_lines: Option<&[usize]>) -> String {
        // Store contract: LF-normalized, BOM-stripped text. Callers may pass
        // raw file bytes (e.g. CRLF restored on write-back); the hash and
        // stored text always describe the normalized form.
        let normalized = normalize_contract(full_text);
        let hash = crate::hash::compute_file_hash(&normalized);
        let key = canonical_key(path);
        let path = key.as_str();

        // Scope the mutable borrow of self.versions so we can touch
        // self.access_order and self.total_bytes afterwards.
        {
            let history = self.versions.entry(path.to_owned()).or_default();

            // Read fusion: same content observed again. Fusion requires BOTH
            // the 16-bit tag AND full-text equality — two distinct texts that
            // happen to share the short tag are DIFFERENT snapshots. Fusing on
            // tag alone would attach seen-lines from text B onto stored text A
            // and let the patcher misresolve the snapshot during recovery /
            // seen-line validation (oh-my-pi issue #4075 analog).
            if let Some(pos) = history
                .iter()
                .position(|s| s.hash == hash && s.text.as_str() == normalized.as_ref())
            {
                let snapshot = &mut history[pos];
                snapshot.recorded_at = now();
                let bytes_before = snapshot_bytes(snapshot);
                merge_seen_lines(snapshot, seen_lines);
                // seen_lines growth is charged against max_total_bytes too.
                self.total_bytes += snapshot_bytes(snapshot) - bytes_before;

                // Promote to front (most recent).
                if pos != 0 {
                    let s = history.remove(pos);
                    history.insert(0, s);
                }
            } else {
                // New snapshot.
                let snapshot = Snapshot {
                    path: path.to_owned(),
                    text: normalized.into_owned(),
                    hash: hash.clone(),
                    recorded_at: now(),
                    seen_lines: seen_lines.map(|lines| lines.iter().copied().collect()),
                };
                self.total_bytes += snapshot_bytes(&snapshot);

                history.insert(0, snapshot);

                // Trim per-path history to max_versions_per_path (oldest
                // dropped first).
                while history.len() > self.max_versions_per_path {
                    let removed = history.pop().unwrap();
                    self.total_bytes = self.total_bytes.saturating_sub(snapshot_bytes(&removed));
                }
            }
        }

        // Bring path to the back of the access order (most recently used).
        if let Some(pos) = self.access_order.iter().position(|p| p.as_str() == path) {
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
        let Some(history) = self.versions.get_mut(&canonical_key(path)) else {
            return;
        };
        // Only the most-recently-recorded version with this tag may receive
        // seen-lines. When two distinct texts collide on a 16-bit tag, the
        // producer just recorded the head (most recent) — attaching lines to an
        // older colliding snapshot would corrupt its provenance (oh-my-pi #4075).
        let Some(snapshot) = history.iter_mut().find(|s| s.hash == hash) else {
            return;
        };
        let bytes_before = snapshot_bytes(snapshot);
        let set = snapshot.seen_lines.get_or_insert_with(HashSet::new);
        for &line in lines {
            set.insert(line);
        }
        self.total_bytes += snapshot_bytes(snapshot) - bytes_before;
    }

    fn invalidate(&mut self, path: &str) {
        let key = canonical_key(path);
        if let Some(history) = self.versions.remove(&key) {
            for s in &history {
                self.total_bytes = self.total_bytes.saturating_sub(snapshot_bytes(s));
            }
        }
        self.access_order.retain(|p| p != &key);
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

    // =====================================================================
    // 16-bit tag collision resolution (oh-my-pi #4075 analog)
    // =====================================================================

    /// Find two distinct texts that share the same 16-bit file tag (top 16
    /// bits of xxh3-64). Used to exercise the collision path deterministically.
    fn find_tag_collision_pair() -> (String, String) {
        use std::collections::HashMap;
        let mut seen: HashMap<String, String> = HashMap::new();
        for i in 0..50_000 {
            let candidate = format!("collision-snapshot-{i}\npayload-{i}\n");
            let tag = crate::hash::compute_file_hash(&candidate);
            if let Some(existing) = seen.insert(tag, candidate.clone()) {
                if existing != candidate {
                    return (existing, candidate);
                }
            }
        }
        panic!("failed to find a 16-bit file-tag collision in search space");
    }

    #[test]
    fn test_read_fusion_requires_full_text_equality() {
        // Two distinct texts sharing a 16-bit tag must NOT fuse: they are
        // different snapshots. Fusing on tag alone would attach seen-lines
        // from text B onto stored text A and misresolve recovery.
        let (text_a, text_b) = find_tag_collision_pair();
        assert_ne!(text_a, text_b);
        assert_eq!(
            crate::hash::compute_file_hash(&text_a),
            crate::hash::compute_file_hash(&text_b),
            "fixture requires a genuine tag collision"
        );

        let mut store = InMemorySnapshotStore::new();
        let tag_a = store.record("/tmp/collide.txt", &text_a, Some(&[1, 2]));
        let tag_b = store.record("/tmp/collide.txt", &text_b, Some(&[3, 4]));

        assert_eq!(tag_a, tag_b, "both texts share the same tag");
        let history = store.versions.get("/tmp/collide.txt").unwrap();
        assert_eq!(history.len(), 2, "colliding texts must be kept separate");
        assert_eq!(history[0].text, text_b, "most recent first");
        assert_eq!(history[1].text, text_a);

        // by_hash resolves the collision to the most-recent matching version.
        let resolved = store.by_hash("/tmp/collide.txt", &tag_a).unwrap();
        assert_eq!(
            resolved.text, text_b,
            "collision resolves to most-recent version"
        );

        // by_content must find each distinct text.
        let a = store.by_content("/tmp/collide.txt", &text_a).unwrap();
        assert_eq!(a.text, text_a);
        let b = store.by_content("/tmp/collide.txt", &text_b).unwrap();
        assert_eq!(b.text, text_b);
    }

    // =====================================================================
    // bd-snapshot-store-z8l: bounds accounting, oversized snapshots,
    // path canonicalization, LF/BOM normalization contract
    // =====================================================================

    #[test]
    fn test_seen_lines_count_toward_total_bytes() {
        let mut store = InMemorySnapshotStore::with_options(30, 4, 1024 * 1024);
        store.record("/tmp/seen.txt", "data", None);
        let before = store.total_byte_count();

        // ~8k distinct line numbers → the HashSet capacity alone is tens of
        // KiB, which must be charged against max_total_bytes.
        let lines: Vec<usize> = (1..=8192).collect();
        store.record_seen_lines(
            "/tmp/seen.txt",
            &store.head("/tmp/seen.txt").unwrap().hash,
            &lines,
        );

        let after = store.total_byte_count();
        assert!(
            after > before + 8192 * std::mem::size_of::<usize>() / 2,
            "seen_lines capacity must be counted: before={before} after={after}"
        );
    }

    #[test]
    fn test_seen_lines_pressure_evicts_oldest_path() {
        // Tiny byte budget. Each snapshot carries a large seen_lines set, so
        // only ~1 path fits; recording a second must evict the first — the
        // sets can no longer grow the store past the cap unbounded.
        let mut store = InMemorySnapshotStore::with_options(30, 4, 4096);
        let lines: Vec<usize> = (1..=2048).collect();

        store.record("/tmp/old.txt", "old", Some(&lines));
        store.record("/tmp/new.txt", "new", Some(&lines));

        assert!(
            store.head("/tmp/old.txt").is_none(),
            "oldest path must be evicted once seen_lines bytes exceed the cap"
        );
        assert!(store.head("/tmp/new.txt").is_some());
    }

    #[test]
    fn test_oversized_snapshot_does_not_evict_itself() {
        // A single snapshot bigger than max_total_bytes must not wipe the
        // whole store (including itself).
        let mut store = InMemorySnapshotStore::with_options(30, 4, 10);

        store.record("/tmp/small.txt", "aaaa", None);
        store.record("/tmp/huge.txt", &"x".repeat(1024), None);

        assert!(store.head("/tmp/small.txt").is_none());
        let huge = store.head("/tmp/huge.txt");
        assert!(huge.is_some(), "oversized snapshot must be retained");
        assert_eq!(huge.unwrap().text.len(), 1024);
    }

    #[test]
    fn test_path_keys_are_canonicalized() {
        let mut store = InMemorySnapshotStore::new();
        store.record("./dir/../a.txt", "v1", None);
        store.record("a\\b.txt", "v2", None);

        // Aliases resolve to the same history.
        assert!(store.head("a.txt").is_some());
        assert_eq!(store.head("./a.txt").unwrap().text, "v1");
        assert!(store.head("a/b.txt").is_some());
        assert_eq!(store.head("a\\b.txt").unwrap().text, "v2");
        assert!(store.head("a\\.\\b.txt").is_some());

        // invalidate via an alias drops the canonical entry.
        store.invalidate("./a.txt");
        assert!(store.head("a.txt").is_none());
        assert!(store.head("a/b.txt").is_some());
    }

    #[test]
    fn test_record_normalizes_crlf_and_bom() {
        let mut store = InMemorySnapshotStore::new();
        let tag = store.record("/tmp/crlf.txt", "a\r\nb\r\n", None);

        let snap = store.head("/tmp/crlf.txt").unwrap();
        assert_eq!(snap.text, "a\nb\n", "stored text must be LF-normalized");
        assert_eq!(
            snap.hash,
            crate::hash::compute_file_hash("a\nb\n"),
            "hash must be over normalized text"
        );
        assert_eq!(tag, snap.hash);

        // by_content accepts raw CRLF bytes too.
        assert!(store.by_content("/tmp/crlf.txt", "a\r\nb\r\n").is_some());
    }

    #[test]
    fn test_record_normalizes_bom() {
        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/bom.txt", "\u{FEFF}hello\n", None);
        assert_eq!(store.head("/tmp/bom.txt").unwrap().text, "hello\n");
    }

    #[test]
    fn test_versions_returns_history_newest_first() {
        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/hist.txt", "v1", None);
        store.record("/tmp/hist.txt", "v2", None);
        store.record("/tmp/hist.txt", "v3", None);

        let versions = store.versions("/tmp/hist.txt");
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].text, "v3");
        assert_eq!(versions[2].text, "v1");
        assert!(store.versions("/tmp/nope.txt").is_empty());
    }

    #[test]
    fn test_record_seen_lines_targets_head_only() {
        // record_seen_lines attaches to the MOST-RECENT version with the tag
        // (the one just recorded). After a colliding text B becomes head,
        // attaching more lines by A's tag lands on B (head), never on the
        // older A — so provenance never corrupts A.
        let (text_a, _text_b) = find_tag_collision_pair();
        let tag = crate::hash::compute_file_hash(&text_a);

        let mut store = InMemorySnapshotStore::new();
        store.record("/tmp/c2.txt", &text_a, Some(&[10]));
        // Text A is head: attaching lines lands on A.
        store.record_seen_lines("/tmp/c2.txt", &tag, &[11]);
        let a = store.by_content("/tmp/c2.txt", &text_a).unwrap();
        let a_seen = a.seen_lines.as_ref().unwrap();
        assert!(a_seen.contains(&10));
        assert!(a_seen.contains(&11));
        assert_eq!(a_seen.len(), 2);
    }
}
