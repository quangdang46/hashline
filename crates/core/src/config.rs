//! Configuration for the hashline Editor.
//!
//! Controls snapshot store behavior, hash algorithm, and resource limits.
//! Used when constructing an [`Editor`](crate::editor::Editor).

/// Configuration for [`Editor`](crate::editor::Editor).
///
/// Controls snapshot caching, hash algorithm, and resource limits
/// for the editor session.
#[derive(Clone, Debug)]
pub struct HashlineConfig {
    /// Whether to enable snapshot caching via the SnapshotStore.
    /// When `false` the Editor skips snapshot recording entirely
    /// (equivalent to a NoopSnapshotStore).
    pub enable_snapshots: bool,

    /// Hash algorithm for line anchors and file fingerprints.
    pub hash_algorithm: HashAlgorithm,

    /// Maximum full-file versions retained per path (default 4).
    pub max_snapshots_per_file: usize,

    /// Maximum distinct paths tracked (default 30).
    pub max_snapshot_paths: usize,

    /// Global ceiling on retained snapshot text across all paths (default 64 MiB).
    pub max_snapshot_bytes: usize,

    /// Whether the no-op loop guard is enabled (default true).
    pub noop_guard_enabled: bool,

    /// Consecutive identical no-op patches before the guard fires (default 3).
    /// A no-op is a patch that produces no net content change on the same path
    /// with the same patch text.
    pub noop_guard_limit: usize,
}

/// Hash algorithm for anchor generation and file fingerprints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HashAlgorithm {
    /// xxHash32 2-char short hashes (current default, fast, collision-resistant).
    #[default]
    Xxh32,
    /// SHA-256 based hashes (behind the `sha256-anchors` feature flag).
    Sha256,
}

impl Default for HashlineConfig {
    fn default() -> Self {
        Self {
            enable_snapshots: true,
            hash_algorithm: HashAlgorithm::default(),
            max_snapshots_per_file: 4,
            max_snapshot_paths: 30,
            max_snapshot_bytes: 64 * 1024 * 1024,
            noop_guard_enabled: true,
            noop_guard_limit: crate::noop_guard::DEFAULT_NOOP_LIMIT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = HashlineConfig::default();
        assert!(cfg.enable_snapshots);
        assert_eq!(cfg.hash_algorithm, HashAlgorithm::Xxh32);
        assert_eq!(cfg.max_snapshots_per_file, 4);
        assert_eq!(cfg.max_snapshot_paths, 30);
        assert_eq!(cfg.max_snapshot_bytes, 64 * 1024 * 1024);
        assert!(cfg.noop_guard_enabled);
        assert_eq!(cfg.noop_guard_limit, crate::noop_guard::DEFAULT_NOOP_LIMIT);
    }

    #[test]
    fn test_hash_algorithm_default() {
        assert_eq!(HashAlgorithm::default(), HashAlgorithm::Xxh32);
    }
}
