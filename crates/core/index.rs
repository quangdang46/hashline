//! Index module — `#[path]` proxies to `src/index/` so `super::` resolves correctly.
#[path = "src/index/mod.rs"]
mod index_module;

pub use index_module::*;
