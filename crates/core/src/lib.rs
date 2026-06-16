//! # Hashline — file-level snapshot-tag based line editing
//!
//! Library entrypoint for embedding hashline in another tool.

#![doc(html_root_url = "https://docs.rs/hashline/0.6.0")]

pub mod types;
pub mod patch_format;
pub mod messages;
pub mod tokenizer;
pub mod parser;
pub mod prefixes;
pub mod normalize;
pub mod hash;
pub mod error;
pub mod document;
pub mod anchor;

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

