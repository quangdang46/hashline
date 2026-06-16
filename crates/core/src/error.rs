#![allow(dead_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HashlineError {
    #[error("{command} is not implemented yet")]
    NotImplemented { command: &'static str },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("file '{path}' is not valid UTF-8")]
    InvalidUtf8 { path: String },

    #[error("file '{path}' appears to be binary and cannot be edited safely")]
    BinaryFile { path: String },

    #[error("file '{path}' uses mixed newline styles")]
    MixedNewlines { path: String },

    #[error("invalid anchor '{anchor}'")]
    InvalidAnchor { anchor: String },

    #[error("invalid range anchor '{range}'")]
    InvalidRange { range: String },

    #[error("hash '{hash}' not found in {path}")]
    HashNotFound { hash: String, path: String },

    #[error("hash '{hash}' matches {count} lines in {path} (lines {lines})")]
    AmbiguousHash {
        hash: String,
        count: usize,
        lines: String,
        path: String,
    },

    #[error(
        "line {line} content changed since last read in {path} (expected hash {expected}, got {actual}){relocated_suffix}"
    )]
    StaleAnchor {
        anchor: Box<str>,
        line: usize,
        expected: Box<str>,
        actual: Box<str>,
        path: Box<str>,
        relocated_suffix: Box<str>,
    },

    #[error("file '{path}' changed since the last read")]
    StaleFile { path: String },

    #[error("invalid indent amount '{amount}' (expected +N or -N)")]
    InvalidIndentAmount { amount: String },

    #[error("range start (line {start}) is after range end (line {end})")]
    InvalidIndentRange { start: usize, end: usize },

    #[error(
        "dedent by {amount} would underflow line {line_no} (only {available} leading {kind} available)"
    )]
    IndentUnderflow {
        line_no: usize,
        amount: usize,
        available: usize,
        kind: &'static str,
    },

    #[error("range uses mixed indentation styles (spaces and tabs) at line {line_no}")]
    MixedIndentation { line_no: usize },

    #[error(
        "could not find balanced block boundary from line {line_no} — check for unmatched braces or inconsistent indentation"
    )]
    UnbalancedBlock { line_no: usize },

    #[error("block language is ambiguous at line {line_no} — use an explicit range anchor instead")]
    AmbiguousBlockLanguage { line_no: usize },

    #[error("invalid pattern '{pattern}': {message}")]
    InvalidPattern { pattern: String, message: String },

    #[error("diff hunk at line {hunk_line} could not be matched to current file content")]
    DiffHunkMismatch { hunk_line: usize },

    #[error("diff targets '{diff_file}' but file argument is '{given_file}'")]
    DiffFileMismatch {
        diff_file: String,
        given_file: String,
    },

    #[error("explode target '{path}' already exists — use --force to overwrite it")]
    ExplodeTargetExists { path: String },

    #[error("implode directory '{path}' is missing .meta.json")]
    ImplodeMissingMeta { path: String },

    #[error("implode metadata in '{path}' is invalid: {reason}")]
    ImplodeInvalidMeta { path: String, reason: String },

    #[error("implode directory '{path}' contains unexpected entry '{entry}'")]
    ImplodeDirtyDirectory { path: String, entry: String },

    #[error("implode directory '{path}' is missing line file for line {line_no}")]
    ImplodeMissingLineFile { path: String, line_no: usize },

    #[error("workflow pack '{path}' is invalid: {reason}")]
    InvalidWorkflowPack { path: String, reason: String },

    #[error("patch failed at operation {op_index}: {reason}")]
    PatchFailed { op_index: usize, reason: String },

    #[error("multi-line content is only supported for range edits")]
    MultiLineContentUnsupported,

    #[error("mutation index {index} is out of bounds for document with {len} lines")]
    MutationIndexOutOfBounds { index: usize, len: usize },

    #[error("mutation range {start}..={end} is invalid for document with {len} lines")]
    InvalidMutationRange {
        start: usize,
        end: usize,
        len: usize,
    },

    #[error("server error: {message}")]
    ServerError { message: String, kind: String },

    #[error(
        "outline input '{path}' is too large to parse safely: {actual} {unit} (limit: {limit} {unit})"
    )]
    OutlineInputTooLarge {
        path: String,
        actual: usize,
        limit: usize,
        unit: &'static str,
    },

    #[error("query '{query}' not found in {path}")]
    QueryNotFound { query: String, path: String },

    #[error("query '{query}' matches {count} lines in {path} (lines {lines})")]
    AmbiguousQuery {
        query: String,
        count: usize,
        lines: String,
        path: String,
    },

    #[error("query range covers {count} lines, exceeds maximum of {max}")]
    QueryRangeTooLarge { count: usize, max: usize },

    /// Free-form error for the SHA-256 backward-compat module
    /// (`sha256_window`). Not used by the native xxh32 path.
    #[cfg(feature = "sha256-anchors")]
    #[error("{0}")]
    Sha256Anchor(String),

    #[error("parse error at line {line}: {message}")]
    ParseError { line: usize, message: String },

    #[error("file not found: '{path}'")]
    FileNotFound { path: String },

    #[error("file '{path}' hash mismatch: expected {expected}, got {actual}")]
    StaleHash {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("cannot recover from inconsistency in '{path}'")]
    CannotRecover { path: String },

    #[error("block unresolved at line {line}: {message}")]
    BlockUnresolved { line: usize, message: String },

    #[error("missing snapshot tag in '{path}'")]
    MissingSnapshotTag { path: String },

    #[error("no block resolver configured")]
    NoBlockResolver,
}

impl HashlineError {
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            HashlineError::NotImplemented { .. } => {
                Some("continue with the next planned implementation bead")
            }
            HashlineError::InvalidUtf8 { .. } => {
                Some("convert the file to UTF-8 before using hashline")
            }
            HashlineError::BinaryFile { .. } => Some("hashline only supports UTF-8 text files"),
            HashlineError::MixedNewlines { .. } => {
                Some("run `dos2unix <file>` or `unix2dos <file>` to normalize first")
            }
            HashlineError::InvalidAnchor { .. } => {
                Some("use a 2-char hash like 'f1' or a qualified anchor like '2:f1'")
            }
            HashlineError::InvalidRange { .. } => Some("use a range like '2:f1..4:9c'"),
            HashlineError::HashNotFound { .. } => {
                Some("run `hashline read <file>` to get current hashes")
            }
            HashlineError::AmbiguousHash { .. } => {
                Some("use a line-qualified hash like '2:f1' to disambiguate")
            }
            HashlineError::StaleAnchor { .. } => Some(
                "re-read the file with `hashline read <file>`; if the hash moved, use the reported line(s) and retry with a fresh qualified anchor",
            ),
            HashlineError::StaleFile { .. } => Some(
                "re-read the file metadata and retry with fresh --expect-mtime/--expect-inode values",
            ),
            HashlineError::InvalidIndentAmount { .. } => {
                Some("use an amount like '+4' to indent or '-2' to dedent")
            }
            HashlineError::InvalidIndentRange { .. } => {
                Some("use a range where the start anchor resolves before the end anchor")
            }
            HashlineError::IndentUnderflow { .. } => {
                Some("reduce the dedent amount or narrow the target range")
            }
            HashlineError::MixedIndentation { .. } => {
                Some("normalize indentation in the target range before retrying the command")
            }
            HashlineError::UnbalancedBlock { .. } => {
                Some("check the surrounding braces or indentation and retry on a well-formed file")
            }
            HashlineError::AmbiguousBlockLanguage { .. } => {
                Some("rename the file to a supported extension or pass an explicit range instead")
            }
            HashlineError::InvalidPattern { .. } => Some("fix the pattern syntax and try again"),
            HashlineError::OutlineInputTooLarge { .. } => Some(
                "use `hashline read --anchor` for a focused view, or run on a smaller file region",
            ),
            HashlineError::DiffHunkMismatch { .. } => {
                Some("re-generate the diff from the current file and retry the command")
            }
            HashlineError::DiffFileMismatch { .. } => {
                Some("check that the diff target matches the file argument and retry")
            }
            HashlineError::ExplodeTargetExists { .. } => {
                Some("remove the output directory first or rerun with --force")
            }
            HashlineError::ImplodeMissingMeta { .. } => Some(
                "run `hashline explode <file> --out <dir>` first or restore the missing .meta.json",
            ),
            HashlineError::ImplodeInvalidMeta { .. } => {
                Some("recreate the exploded directory from a fresh `hashline explode` and retry")
            }
            HashlineError::ImplodeDirtyDirectory { .. } => {
                Some("remove unexpected files from the explode directory and retry the implode")
            }
            HashlineError::ImplodeMissingLineFile { .. } => Some(
                "restore the missing line file or regenerate the explode directory before retrying",
            ),
            HashlineError::InvalidWorkflowPack { .. } => {
                Some("fix the markdown frontmatter fields in the workflow pack and retry")
            }
            HashlineError::PatchFailed { .. } => {
                Some("fix the failing patch operation and retry the transaction")
            }
            HashlineError::MultiLineContentUnsupported => Some(
                "use a range anchor like '2:f1..4:9c' for multi-line replacement, or use `hashline patch` for mixed edits",
            ),
            HashlineError::MutationIndexOutOfBounds { .. } => {
                Some("re-check the resolved line number against the current document and retry")
            }
            HashlineError::InvalidMutationRange { .. } => {
                Some("use a valid in-bounds range where the start line is not after the end line")
            }
            HashlineError::ServerError { .. } => {
                Some("ensure the daemon is running with `hashline daemon`")
            }
            HashlineError::QueryNotFound { .. } => {
                Some("check the query text against the file content and retry")
            }
            HashlineError::AmbiguousQuery { .. } => Some(
                "use a more specific query that matches exactly one line, or use an explicit anchor instead",
            ),
            HashlineError::QueryRangeTooLarge { .. } => {
                Some("narrow the query range by using a more specific start-query or end-query")
            }
            HashlineError::Io(_) => {
                Some("check the file path and permissions, then retry the command")
            }
            HashlineError::Json(_) => {
                Some("fix the JSON input or output handling and retry the command")
            }
            HashlineError::ParseError { .. } => {
                Some("check the input syntax around the reported line and retry")
            }
            HashlineError::FileNotFound { .. } => {
                Some("verify the file path exists and is accessible")
            }
            HashlineError::StaleHash { .. } => {
                Some("re-read the file with `hashline read <file>` to get current hashes")
            }
            HashlineError::CannotRecover { .. } => {
                Some("examine the file for structural issues and consider manual repair")
            }
            HashlineError::BlockUnresolved { .. } => {
                Some("check the block boundaries around the reported line and retry")
            }
            HashlineError::MissingSnapshotTag { .. } => {
                Some("ensure the file contains a snapshot tag marker")
            }
            HashlineError::NoBlockResolver => {
                Some("configure a block resolver before using block-based operations")
            }
            #[cfg(feature = "sha256-anchors")]
            HashlineError::Sha256Anchor(_) => Some(
                "use the `sha256_window` module to recompute the expected hash from current content",
            ),
        }
    }

    pub fn command(&self) -> Option<&'static str> {
        match self {
            HashlineError::NotImplemented { command } => Some(command),
            HashlineError::Io(_)
            | HashlineError::Json(_)
            | HashlineError::InvalidUtf8 { .. }
            | HashlineError::BinaryFile { .. }
            | HashlineError::MixedNewlines { .. }
            | HashlineError::InvalidAnchor { .. }
            | HashlineError::InvalidRange { .. }
            | HashlineError::HashNotFound { .. }
            | HashlineError::AmbiguousHash { .. }
            | HashlineError::StaleAnchor { .. }
            | HashlineError::StaleFile { .. }
            | HashlineError::InvalidIndentAmount { .. }
            | HashlineError::InvalidIndentRange { .. }
            | HashlineError::IndentUnderflow { .. }
            | HashlineError::MixedIndentation { .. }
            | HashlineError::UnbalancedBlock { .. }
            | HashlineError::AmbiguousBlockLanguage { .. }
            | HashlineError::InvalidPattern { .. }
            | HashlineError::OutlineInputTooLarge { .. }
            | HashlineError::DiffHunkMismatch { .. }
            | HashlineError::DiffFileMismatch { .. }
            | HashlineError::ExplodeTargetExists { .. }
            | HashlineError::ImplodeMissingMeta { .. }
            | HashlineError::ImplodeInvalidMeta { .. }
            | HashlineError::ImplodeDirtyDirectory { .. }
            | HashlineError::ImplodeMissingLineFile { .. }
            | HashlineError::InvalidWorkflowPack { .. }
            | HashlineError::PatchFailed { .. }
            | HashlineError::MultiLineContentUnsupported
            | HashlineError::MutationIndexOutOfBounds { .. }
            | HashlineError::InvalidMutationRange { .. }
            | HashlineError::ServerError { .. }
            | HashlineError::QueryNotFound { .. }
            | HashlineError::AmbiguousQuery { .. }
            | HashlineError::QueryRangeTooLarge { .. }
            | HashlineError::ParseError { .. }
            | HashlineError::FileNotFound { .. }
            | HashlineError::StaleHash { .. }
            | HashlineError::CannotRecover { .. }
            | HashlineError::BlockUnresolved { .. }
            | HashlineError::MissingSnapshotTag { .. }
            | HashlineError::NoBlockResolver => None,
            #[cfg(feature = "sha256-anchors")]
            HashlineError::Sha256Anchor(_) => None,
        }
    }

    pub fn log_as_error(&self) -> bool {
        matches!(
            self,
            HashlineError::NotImplemented { .. }
                | HashlineError::MutationIndexOutOfBounds { .. }
                | HashlineError::InvalidMutationRange { .. }
        )
    }

    /// Returns `true` if this is a [`StaleAnchor`] error, meaning the
    /// file content changed since the anchor was fetched. Retrying after
    /// a forced re-read may resolve it.
    pub fn is_stale_anchor(&self) -> bool {
        matches!(self, HashlineError::StaleAnchor { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::HashlineError;

    #[test]
    fn every_error_variant_has_a_recovery_hint() {
        let errors = vec![
            HashlineError::NotImplemented { command: "patch" },
            HashlineError::Io(std::io::Error::other("boom")),
            HashlineError::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
            HashlineError::InvalidUtf8 {
                path: "demo.txt".into(),
            },
            HashlineError::BinaryFile {
                path: "demo.bin".into(),
            },
            HashlineError::MixedNewlines {
                path: "demo.txt".into(),
            },
            HashlineError::InvalidAnchor {
                anchor: "bogus".into(),
            },
            HashlineError::InvalidRange {
                range: "1:aa..0:bb".into(),
            },
            HashlineError::HashNotFound {
                hash: "ff".into(),
                path: "demo.txt".into(),
            },
            HashlineError::AmbiguousHash {
                hash: "aa".into(),
                count: 2,
                lines: "1, 3".into(),
                path: "demo.txt".into(),
            },
            HashlineError::StaleAnchor {
                anchor: "2:aa".into(),
                line: 2,
                expected: "aa".into(),
                actual: "bb".into(),
                path: "demo.txt".into(),
                relocated_suffix: "".into(),
            },
            HashlineError::StaleFile {
                path: "demo.txt".into(),
            },
            HashlineError::InvalidIndentAmount {
                amount: "sideways".into(),
            },
            HashlineError::InvalidIndentRange { start: 4, end: 2 },
            HashlineError::IndentUnderflow {
                line_no: 2,
                amount: 2,
                available: 1,
                kind: "spaces",
            },
            HashlineError::MixedIndentation { line_no: 3 },
            HashlineError::UnbalancedBlock { line_no: 8 },
            HashlineError::AmbiguousBlockLanguage { line_no: 5 },
            HashlineError::InvalidPattern {
                pattern: "(".into(),
                message: "unclosed group".into(),
            },
            HashlineError::DiffHunkMismatch { hunk_line: 12 },
            HashlineError::DiffFileMismatch {
                diff_file: "a/demo.txt".into(),
                given_file: "demo.txt".into(),
            },
            HashlineError::ExplodeTargetExists {
                path: "out/dir".into(),
            },
            HashlineError::ImplodeMissingMeta {
                path: "out/dir".into(),
            },
            HashlineError::ImplodeInvalidMeta {
                path: "out/dir/.meta.json".into(),
                reason: "missing newline".into(),
            },
            HashlineError::ImplodeDirtyDirectory {
                path: "out/dir".into(),
                entry: "notes.txt".into(),
            },
            HashlineError::ImplodeMissingLineFile {
                path: "out/dir".into(),
                line_no: 2,
            },
            HashlineError::PatchFailed {
                op_index: 1,
                reason: "bad op".into(),
            },
            HashlineError::MultiLineContentUnsupported,
            HashlineError::MutationIndexOutOfBounds { index: 5, len: 2 },
            HashlineError::InvalidMutationRange {
                start: 3,
                end: 1,
                len: 2,
            },
            HashlineError::ServerError {
                message: "connection refused".into(),
                kind: "not_running".into(),
            },
            HashlineError::QueryNotFound {
                query: "fn main".into(),
                path: "demo.txt".into(),
            },
            HashlineError::AmbiguousQuery {
                query: "println".into(),
                count: 3,
                lines: "5, 10, 15".into(),
                path: "demo.txt".into(),
            },
            HashlineError::QueryRangeTooLarge {
                count: 15000,
                max: 10000,
            },
            HashlineError::ParseError {
                line: 42,
                message: "unexpected token".into(),
            },
            HashlineError::FileNotFound {
                path: "missing.txt".into(),
            },
            HashlineError::StaleHash {
                path: "demo.txt".into(),
                expected: "aa".into(),
                actual: "bb".into(),
            },
            HashlineError::CannotRecover {
                path: "broken.txt".into(),
            },
            HashlineError::BlockUnresolved {
                line: 15,
                message: "mismatched braces".into(),
            },
            HashlineError::MissingSnapshotTag {
                path: "snapshot.txt".into(),
            },
            HashlineError::NoBlockResolver,
        ];

        for error in errors {
            assert!(
                error.hint().is_some(),
                "expected a recovery hint for error variant: {error:?}"
            );
        }
    }

    #[test]
    fn not_implemented_reports_command_name() {
        let error = HashlineError::NotImplemented { command: "patch" };
        assert_eq!(error.command(), Some("patch"));
    }

    #[test]
    fn stale_anchor_hint_mentions_relocated_lines() {
        let error = HashlineError::StaleAnchor {
            anchor: "2:aa".into(),
            line: 2,
            expected: "aa".into(),
            actual: "bb".into(),
            path: "demo.txt".into(),
            relocated_suffix: "; hash still exists at line(s) 9".into(),
        };

        assert_eq!(
            error.hint(),
            Some(
                "re-read the file with `hashline read <file>`; if the hash moved, use the reported line(s) and retry with a fresh qualified anchor"
            )
        );
        assert!(error.to_string().contains("hash still exists at line(s) 9"));
    }

    #[test]
    fn recoverable_validation_errors_do_not_log_as_error() {
        let error = HashlineError::StaleAnchor {
            anchor: "2:aa".into(),
            line: 2,
            expected: "aa".into(),
            actual: "bb".into(),
            path: "demo.txt".into(),
            relocated_suffix: "".into(),
        };

        assert!(!error.log_as_error());
    }

    #[test]
    fn invariant_failures_log_as_error() {
        assert!(
            HashlineError::InvalidMutationRange {
                start: 3,
                end: 1,
                len: 2,
            }
            .log_as_error()
        );
    }

    #[test]
    fn implode_errors_have_recovery_hints() {
        assert!(
            HashlineError::ImplodeMissingMeta { path: "out".into() }
                .hint()
                .is_some()
        );
        assert!(
            HashlineError::ImplodeInvalidMeta {
                path: "out/.meta.json".into(),
                reason: "bad".into()
            }
            .hint()
            .is_some()
        );
        assert!(
            HashlineError::ImplodeDirtyDirectory {
                path: "out".into(),
                entry: "notes.txt".into()
            }
            .hint()
            .is_some()
        );
        assert!(
            HashlineError::ImplodeMissingLineFile {
                path: "out".into(),
                line_no: 2
            }
            .hint()
            .is_some()
        );
    }
}
