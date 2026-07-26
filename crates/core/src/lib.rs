//! # Hashline — file-level snapshot-tag based line editing
//!
//! Library entrypoint for embedding hashline in another tool.

#![doc(html_root_url = "https://docs.rs/hashline/0.6.0")]
#![allow(clippy::needless_range_loop, clippy::ptr_arg, clippy::needless_return)]

pub mod anchor;
pub mod apply;
pub mod block;
pub mod builtin_resolver;
pub mod config;
pub mod document;
pub mod editor;
pub mod error;
pub mod hash;
pub mod merge;
pub mod messages;
pub mod normalize;
pub mod parser;
pub mod patch_format;
pub mod prefixes;
pub mod recovery;
pub mod snapshot_store;
pub mod tokenizer;
pub mod types;

#[cfg(feature = "sha256-anchors")]
pub mod sha256_window;

#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod commands;
#[doc(hidden)]
pub mod context;
#[doc(hidden)]
pub mod mcp;
#[doc(hidden)]
pub mod orchestration;
#[doc(hidden)]
pub mod output;

// Re-exports: the public API surface for library consumers
pub use config::HashlineConfig;
pub use editor::{Editor, FindBlockResult, LineWithHash, PatchResult, ReadResult, WriteResult};
pub use snapshot_store::SnapshotStore;
pub use types::BlockResolver;
