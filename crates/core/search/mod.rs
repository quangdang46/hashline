//! Trigram search module for instant grep.

pub mod cache;
pub mod decompose;
pub mod extract;
pub mod filter;
pub mod index;
pub mod persist;
pub mod types;
pub mod verify;

pub use cache::{CacheStats, IndexCache, SharedIndexCache};
pub use types::{IndexMeta, IndexStats, LocMask, NextMask, Posting, Trigram, TrigramIndex};
