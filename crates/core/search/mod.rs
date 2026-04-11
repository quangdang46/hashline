//! Trigram search module for instant grep.

pub mod cache;
pub mod decompose;
pub mod extract;
pub mod filter;
pub mod index;
pub mod persist;
pub mod types;
pub mod verify;

// Re-exported for API completeness - items may be unused by internal code
// but are part of the public interface
#[allow(unused)]
pub use cache::{CacheStats, IndexCache, SharedIndexCache};
#[allow(unused)]
pub use types::{IndexMeta, IndexStats, LocMask, NextMask, Posting, Trigram, TrigramIndex};
