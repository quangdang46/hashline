#![allow(dead_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LinehashError {
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
        "could not find balanced block boundary from line {line_no} — check for unmatched braces"
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
}

impl LinehashError {
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            LinehashError::NotImplemented { .. } => {
                Some("continue with the next planned implementation bead")
            }
            LinehashError::InvalidUtf8 { .. } => {
                Some("convert the file to UTF-8 before using linehash")
            }
            LinehashError::BinaryFile { .. } => Some("linehash only supports UTF-8 text files"),
            LinehashError::MixedNewlines { .. } => {
                Some("run `dos2unix <file>` or `unix2dos <file>` to normalize first")
            }
            LinehashError::InvalidAnchor { .. } => {
                Some("use a 2-char hash like 'f1' or a qualified anchor like '2:f1'")
            }
            LinehashError::InvalidRange { .. } => Some("use a range like '2:f1..4:9c'"),
            LinehashError::HashNotFound { .. } => {
                Some("run `linehash read <file>` to get current hashes")
            }
            LinehashError::AmbiguousHash { .. } => {
                Some("use a line-qualified hash like '2:f1' to disambiguate")
            }
            LinehashError::StaleAnchor { .. } => Some(
                "re-read the file with `linehash read <file>`; if the hash moved, use the reported line(s) and retry with a fresh qualified anchor",
            ),
            LinehashError::StaleFile { .. } => Some(
                "re-read the file metadata and retry with fresh --expect-mtime/--expect-inode values",
            ),
            LinehashError::InvalidIndentAmount { .. } => {
                Some("use an amount like '+4' to indent or '-2' to dedent")
            }
            LinehashError::InvalidIndentRange { .. } => {
                Some("use a range where the start anchor resolves before the end anchor")
            }
            LinehashError::IndentUnderflow { .. } => {
                Some("reduce the dedent amount or narrow the target range")
            }
            LinehashError::MixedIndentation { .. } => {
                Some("normalize indentation in the target range before retrying the command")
            }
            LinehashError::UnbalancedBlock { .. } => Some(
                "check the surrounding braces or block delimiters and retry on a well-formed file",
            ),
            LinehashError::AmbiguousBlockLanguage { .. } => {
                Some("rename the file to a supported extension or pass an explicit range instead")
            }
            LinehashError::InvalidPattern { .. } => Some("fix the pattern syntax and try again"),
            LinehashError::DiffHunkMismatch { .. } => {
                Some("re-generate the diff from the current file and retry the command")
            }
            LinehashError::DiffFileMismatch { .. } => {
                Some("check that the diff target matches the file argument and retry")
            }
            LinehashError::ExplodeTargetExists { .. } => {
                Some("remove the output directory first or rerun with --force")
            }
            LinehashError::ImplodeMissingMeta { .. } => Some(
                "run `linehash explode <file> --out <dir>` first or restore the missing .meta.json",
            ),
            LinehashError::ImplodeInvalidMeta { .. } => {
                Some("recreate the exploded directory from a fresh `linehash explode` and retry")
            }
            LinehashError::ImplodeDirtyDirectory { .. } => {
                Some("remove unexpected files from the explode directory and retry the implode")
            }
            LinehashError::ImplodeMissingLineFile { .. } => Some(
                "restore the missing line file or regenerate the explode directory before retrying",
            ),
            LinehashError::PatchFailed { .. } => {
                Some("fix the failing patch operation and retry the transaction")
            }
            LinehashError::MultiLineContentUnsupported => Some(
                "use a range anchor like '2:f1..4:9c' for multi-line replacement, or use `linehash patch` for mixed edits",
            ),
            LinehashError::MutationIndexOutOfBounds { .. } => {
                Some("re-check the resolved line number against the current document and retry")
            }
            LinehashError::InvalidMutationRange { .. } => {
                Some("use a valid in-bounds range where the start line is not after the end line")
            }
            LinehashError::Io(_) => {
                Some("check the file path and permissions, then retry the command")
            }
            LinehashError::Json(_) => {
                Some("fix the JSON input or output handling and retry the command")
            }
        }
    }

    pub fn command(&self) -> Option<&'static str> {
        match self {
            LinehashError::NotImplemented { command } => Some(command),
            LinehashError::Io(_)
            | LinehashError::Json(_)
            | LinehashError::InvalidUtf8 { .. }
            | LinehashError::BinaryFile { .. }
            | LinehashError::MixedNewlines { .. }
            | LinehashError::InvalidAnchor { .. }
            | LinehashError::InvalidRange { .. }
            | LinehashError::HashNotFound { .. }
            | LinehashError::AmbiguousHash { .. }
            | LinehashError::StaleAnchor { .. }
            | LinehashError::StaleFile { .. }
            | LinehashError::InvalidIndentAmount { .. }
            | LinehashError::InvalidIndentRange { .. }
            | LinehashError::IndentUnderflow { .. }
            | LinehashError::MixedIndentation { .. }
            | LinehashError::UnbalancedBlock { .. }
            | LinehashError::AmbiguousBlockLanguage { .. }
            | LinehashError::InvalidPattern { .. }
            | LinehashError::DiffHunkMismatch { .. }
            | LinehashError::DiffFileMismatch { .. }
            | LinehashError::ExplodeTargetExists { .. }
            | LinehashError::ImplodeMissingMeta { .. }
            | LinehashError::ImplodeInvalidMeta { .. }
            | LinehashError::ImplodeDirtyDirectory { .. }
            | LinehashError::ImplodeMissingLineFile { .. }
            | LinehashError::PatchFailed { .. }
            | LinehashError::MultiLineContentUnsupported
            | LinehashError::MutationIndexOutOfBounds { .. }
            | LinehashError::InvalidMutationRange { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LinehashError;

    #[test]
    fn every_error_variant_has_a_recovery_hint() {
        let errors = vec![
            LinehashError::NotImplemented { command: "patch" },
            LinehashError::Io(std::io::Error::other("boom")),
            LinehashError::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
            LinehashError::InvalidUtf8 {
                path: "demo.txt".into(),
            },
            LinehashError::BinaryFile {
                path: "demo.bin".into(),
            },
            LinehashError::MixedNewlines {
                path: "demo.txt".into(),
            },
            LinehashError::InvalidAnchor {
                anchor: "bogus".into(),
            },
            LinehashError::InvalidRange {
                range: "1:aa..0:bb".into(),
            },
            LinehashError::HashNotFound {
                hash: "ff".into(),
                path: "demo.txt".into(),
            },
            LinehashError::AmbiguousHash {
                hash: "aa".into(),
                count: 2,
                lines: "1, 3".into(),
                path: "demo.txt".into(),
            },
            LinehashError::StaleAnchor {
                anchor: "2:aa".into(),
                line: 2,
                expected: "aa".into(),
                actual: "bb".into(),
                path: "demo.txt".into(),
                relocated_suffix: "".into(),
            },
            LinehashError::StaleFile {
                path: "demo.txt".into(),
            },
            LinehashError::InvalidIndentAmount {
                amount: "sideways".into(),
            },
            LinehashError::InvalidIndentRange { start: 4, end: 2 },
            LinehashError::IndentUnderflow {
                line_no: 2,
                amount: 2,
                available: 1,
                kind: "spaces",
            },
            LinehashError::MixedIndentation { line_no: 3 },
            LinehashError::UnbalancedBlock { line_no: 8 },
            LinehashError::AmbiguousBlockLanguage { line_no: 5 },
            LinehashError::InvalidPattern {
                pattern: "(".into(),
                message: "unclosed group".into(),
            },
            LinehashError::DiffHunkMismatch { hunk_line: 12 },
            LinehashError::DiffFileMismatch {
                diff_file: "a/demo.txt".into(),
                given_file: "demo.txt".into(),
            },
            LinehashError::ExplodeTargetExists {
                path: "out/dir".into(),
            },
            LinehashError::ImplodeMissingMeta {
                path: "out/dir".into(),
            },
            LinehashError::ImplodeInvalidMeta {
                path: "out/dir/.meta.json".into(),
                reason: "missing newline".into(),
            },
            LinehashError::ImplodeDirtyDirectory {
                path: "out/dir".into(),
                entry: "notes.txt".into(),
            },
            LinehashError::ImplodeMissingLineFile {
                path: "out/dir".into(),
                line_no: 2,
            },
            LinehashError::PatchFailed {
                op_index: 1,
                reason: "bad op".into(),
            },
            LinehashError::MultiLineContentUnsupported,
            LinehashError::MutationIndexOutOfBounds { index: 5, len: 2 },
            LinehashError::InvalidMutationRange {
                start: 3,
                end: 1,
                len: 2,
            },
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
        let error = LinehashError::NotImplemented { command: "patch" };
        assert_eq!(error.command(), Some("patch"));
    }

    #[test]
    fn stale_anchor_hint_mentions_relocated_lines() {
        let error = LinehashError::StaleAnchor {
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
                "re-read the file with `linehash read <file>`; if the hash moved, use the reported line(s) and retry with a fresh qualified anchor"
            )
        );
        assert!(error.to_string().contains("hash still exists at line(s) 9"));
    }

    #[test]
    fn implode_errors_have_recovery_hints() {
        assert!(
            LinehashError::ImplodeMissingMeta { path: "out".into() }
                .hint()
                .is_some()
        );
        assert!(
            LinehashError::ImplodeInvalidMeta {
                path: "out/.meta.json".into(),
                reason: "bad".into()
            }
            .hint()
            .is_some()
        );
        assert!(
            LinehashError::ImplodeDirtyDirectory {
                path: "out".into(),
                entry: "notes.txt".into()
            }
            .hint()
            .is_some()
        );
        assert!(
            LinehashError::ImplodeMissingLineFile {
                path: "out".into(),
                line_no: 2
            }
            .hint()
            .is_some()
        );
    }
}
