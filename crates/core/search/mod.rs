//! Trigram search module for instant grep.

pub mod extract;
pub mod index;
pub mod persist;
pub mod types;

pub use types::{IndexMeta, LocMask, NextMask, Posting, Trigram, TrigramIndex};
