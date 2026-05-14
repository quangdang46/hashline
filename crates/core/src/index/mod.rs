// These modules contain scaffolding for the new search-index pipeline. Most
// items are not yet wired into the CLI / MCP paths but are exercised by the
// benchmark suite (which compiles each module via a `#[path = "..."]` shim)
// and are kept public so downstream consumers can depend on them as the
// pipeline is rolled out incrementally.
#![allow(dead_code)]

pub mod adaptive;
pub mod cache;
pub mod persist;
pub mod token;
pub mod trigram;
pub mod types;
pub mod zone;
