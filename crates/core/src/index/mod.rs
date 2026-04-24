pub mod adaptive;
pub mod cache;
pub mod persist;
pub mod token;
pub mod trigram;
pub mod types;
pub mod zone;

pub use adaptive::{PatternType, SearchResult, classify_pattern, search_adaptive};
pub use cache::{IndexCache, IndexCacheEntry, SharedIndexCache};
pub use token::{LineBitSet, TokenIndex};
pub use trigram::{Trigram, TrigramIndex};
pub use types::IndexStats;
pub use zone::{ZONE_SIZE_BYTES, Zone, ZoneMap};
