//! Pure data types shared across the hashline parser, applier, and patcher.
//! Nothing in this file references a filesystem — keep it that way.

use std::collections::HashMap;

use serde::Serialize;

/// A file-level operation that applies to the whole file, not individual lines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileOp {
    /// Delete the file entirely. No payload.
    Remove,
    /// Rename (move) the file to the given destination path.
    Rename(String),
}

/// A line-number anchor (1-indexed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Anchor {
    pub line: usize,
}

/// Where an `insert` edit should land relative to existing content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cursor {
    Bof,
    Eof,
    BeforeAnchor(Anchor),
    AfterAnchor(Anchor),
}

/// Insert mode distinguishing replacement inserts from plain inserts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertMode {
    Replacement,
}

/// Block edit mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockMode {
    InsertAfter,
    InsertBefore,
}

/// A single low-level edit produced by the parser and consumed by the applier.
/// Multi-line replacements decompose to one `Insert` per replacement line plus
/// one `Delete` per consumed line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Edit {
    Insert {
        cursor: Cursor,
        text: String,
        line_num: usize,
        index: usize,
        mode: Option<InsertMode>,
        /// Present on inserts lowered from `insert_after_block N:`: the
        /// resolved block's first line. Lets the applier slide a body that
        /// claims a depth inside the block back across the block's trailing
        /// closer lines (never above this line).
        block_start: Option<usize>,
        /// Optional short-hash expected at the anchor line, from `SWAP N:HH:`.
        expected_hash: Option<u8>,
    },
    Delete {
        anchor: Anchor,
        line_num: usize,
        index: usize,
        /// Optional short-hash expected at the anchor line, from `SWAP N:HH:`.
        expected_hash: Option<u8>,
    },
    /// Deferred block edit (`replace_block N:` / `delete_block N` /
    /// `insert_after_block N:`). The exact line span is unknown at parse
    /// time — it is resolved once file text + path are available.
    /// `apply_edits` never sees this variant.
    Block {
        anchor: Anchor,
        payloads: Vec<String>,
        line_num: usize,
        index: usize,
        mode: Option<BlockMode>,
        /// Optional short-hash expected at the anchor line, from e.g. `SWAP.BLK 4:ff:`.
        expected_hash: Option<u8>,
    },
    /// `CUT N..=M @name` — capture original lines N..=M into a named
    /// register (or the anonymous register when `register` is `None`) and
    /// delete them from the file. Captured text is stored in the
    /// per-patch clipboard at apply time, keyed by the register name.
    Cut {
        anchor: Anchor,
        end: Anchor,
        line_num: usize,
        index: usize,
        register: Option<String>,
        /// Optional short-hash expected at the first anchor line, from e.g. `CUT 5:aa..9:`.
        expected_hash: Option<u8>,
    },
    /// `PUT @name <N:` — insert the lines captured earlier in the same
    /// patch into a named (or anonymous) register before line `N`.
    /// A register that was never captured is a hard error at apply time.
    Paste {
        cursor: Cursor,
        line_num: usize,
        index: usize,
        register: Option<String>,
    },
}

/// Result of applying a parsed set of edits to a text body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyResult {
    /// Post-edit text body.
    pub text: String,
    /// First line number (1-indexed) that changed, or `None` for a no-op apply.
    pub first_changed_line: Option<usize>,
    /// Diagnostic warnings collected by the parser, applier, or recovery.
    pub warnings: Vec<String>,
    /// Resolved spans for each `replace_block`/`delete_block` op in this apply,
    /// in patch order. Present only when the apply matched the tagged content
    /// (the common no-drift path).
    pub block_resolutions: Vec<BlockResolution>,
}

/// A parsed `[A..=B]` line range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParsedRange {
    pub start: Anchor,
    pub end: Anchor,
}

/// Resolved 1-indexed inclusive line span of a block target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockSpan {
    pub start: usize,
    pub end: usize,
}

/// One block anchor resolved to its concrete line span.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BlockResolution {
    /// The 1-indexed line the block op was anchored on (the `N`).
    pub anchor_line: usize,
    /// First line of the resolved span (1-indexed, inclusive).
    pub start: usize,
    /// Last line of the resolved span (1-indexed, inclusive).
    pub end: usize,
    /// Which block op produced this resolution.
    pub op: BlockOp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum BlockOp {
    Replace,
    Delete,
    InsertAfter,
    InsertBefore,
}

/// Per-patch clipboard carrying lines captured by `CUT` ops so later `PUT`
/// ops in the same patch can paste them. Lives for the duration of a single
/// patch application and does not persist across patch calls.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Clipboard {
    /// Named registers, keyed by `@name`.
    pub named: HashMap<String, Vec<String>>,
    /// Anonymous register, filled by a `CUT` without `@name`.
    pub anon: Option<Vec<String>>,
}

/// Request handed to a [`BlockResolver`] to resolve one block anchor.
#[derive(Clone, Debug)]
pub struct BlockResolverRequest {
    /// Target file path (used to infer language by extension).
    pub path: String,
    /// Full text the block must be resolved against.
    pub text: String,
    /// 1-indexed line the block must begin on.
    pub line: usize,
}

/// Resolves a block anchor to the line span of the syntactic block
/// that begins on line N. Returns `None` when no block can be resolved.
/// Pure seam: the hashline core declares the contract; the host injects
/// an implementation (tree-sitter, brace-matching, etc.).
pub trait BlockResolver: Send + Sync {
    fn resolve(&self, request: &BlockResolverRequest) -> Option<BlockSpan>;
}

/// Optional hints for [`Patch::parse`](crate::input::Patch::parse).
#[derive(Clone, Debug, Default)]
pub struct SplitOptions {
    /// Resolves absolute paths inside hashline headers to cwd-relative form.
    pub cwd: Option<String>,
    /// Fallback path used when the input lacks a `[PATH]` header but contains
    /// recognizable hashline operations.
    pub path: Option<String>,
}

/// Streaming-formatter knobs.
#[derive(Clone, Debug)]
pub struct StreamOptions {
    /// First line number to use when formatting (1-indexed, default 1).
    pub start_line: usize,
    /// Maximum formatted lines per yielded chunk (default 200).
    pub max_chunk_lines: usize,
    /// Maximum UTF-8 bytes per yielded chunk (default 64 KiB).
    pub max_chunk_bytes: usize,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            start_line: 1,
            max_chunk_lines: 200,
            max_chunk_bytes: 64 * 1024,
        }
    }
}
