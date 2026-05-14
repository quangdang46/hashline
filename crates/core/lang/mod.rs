// Language-aware analysis scaffolding. Most items here are public surface
// for the new callgraph / deps / symbol pipeline that has not yet been wired
// into the CLI / MCP entry points. They are kept here so the benchmark
// suite (which compiles each module via `#[path = "..."]`) can exercise
// them and so callers can opt in incrementally.
#![allow(dead_code)]

pub mod callgraph;
pub mod deps;
pub mod detect;
pub mod outline;
pub mod signature;
pub mod symbol;
