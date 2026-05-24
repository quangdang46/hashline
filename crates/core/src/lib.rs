//! # Hashline — hash-anchored line editing
//!
//! Library entrypoint for embedding hashline in another tool. The
//! `hashline` CLI binary is a thin wrapper over these modules.
//!
//! ## Quick start
//!
//! ```no_run
//! use std::path::Path;
//! use hashline::{document::Document, anchor::{parse_anchor, resolve_without_index}};
//!
//! let doc = Document::load(Path::new("src/lib.rs")).unwrap();
//! let anchor = parse_anchor("42:ab").unwrap();
//! let resolved = resolve_without_index(&anchor, &doc).unwrap();
//! println!("Anchor resolves to line {}", resolved.line_no);
//! ```
//!
//! ## Stability
//!
//! Modules at the crate root that have a doc comment (e.g. [`anchor`],
//! [`document`], [`hash`], [`mutation`]) are part of the public,
//! semver-stable API. Modules tagged with `#[doc(hidden)]`
//! ([`cli`], [`commands`], [`context`], [`mcp`], [`orchestration`],
//! [`output`], [`receipt`], [`risk`]) back the CLI binary and may
//! change between minor versions; depend on them at your own risk.

#![doc(html_root_url = "https://docs.rs/hashline/0.2.0")]

// ---- Public, stable API ----

pub mod anchor;
pub mod document;
pub mod error;
pub mod hash;
pub mod hash_cache;
pub mod mutation;

/// SHA-256 window-hash anchors (backward-compat with jcode and other
/// tools that pre-date hashline's xxh32 anchor format). Available
/// only with the `sha256-anchors` feature enabled.
#[cfg(feature = "sha256-anchors")]
pub mod sha256_window;

// ---- Binary-supporting internals (public for the bin, not for consumers) ----

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
#[doc(hidden)]
pub mod receipt;
#[doc(hidden)]
pub mod risk;
