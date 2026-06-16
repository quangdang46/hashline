//! Error type raised when a section's snapshot tag does not match the live file
//! content and recovery is unavailable / has failed.
//!
//! Carries enough context to render a useful diagnostic: the anchored lines
//! plus a couple of lines of surrounding context. The [`MismatchError`]
//! formats this into a message through [`Display`].

use std::fmt;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::patch_format::{
    format_hashline_header, format_numbered_line, HL_FILE_HASH_EXAMPLES, HL_FILE_HASH_SEP,
    HL_FILE_PREFIX, HL_FILE_SUFFIX,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Lines of context shown either side of a hash mismatch.
pub const MISMATCH_CONTEXT: usize = 2;

// ---------------------------------------------------------------------------
// Regex
// ---------------------------------------------------------------------------

static LINE_REF_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*[>+\-*]*\s*(\d+)(?::.*)?\s*$").unwrap());

// ---------------------------------------------------------------------------
// Helper: format_full_anchor_requirement
// ---------------------------------------------------------------------------

/// Format the required-shape diagnostic shown when a line reference is malformed.
pub fn format_full_anchor_requirement(raw: Option<&str>) -> String {
    let received = match raw {
        Some(s) => format!(". Received {s:?}"),
        None => String::new(),
    };
    format!(
        "a bare line number from read/search output plus the section header content-hash tag \
         (for example {} and line \"160\"){received}",
        format_hashline_header("src/foo.ts", HL_FILE_HASH_EXAMPLES[0])
    )
}

// ---------------------------------------------------------------------------
// ParsedTag + parse_tag
// ---------------------------------------------------------------------------

/// A parsed bare line-number anchor like `42`, `*42:foo`, ` > 7`.
#[derive(Clone, Copy, Debug)]
pub struct ParsedTag {
    pub line: usize,
}

/// Parse a decorated bare line-number anchor such as `42`, `*42:foo`, or ` > 7`.
///
/// Returns an `Err` with a human-readable message when the reference does not
/// match the expected format.
pub fn parse_tag(ref_text: &str) -> Result<ParsedTag, String> {
    let caps = LINE_REF_RE.captures(ref_text).ok_or_else(|| {
        format!(
            "Invalid line reference. Expected {}.",
            format_full_anchor_requirement(Some(ref_text))
        )
    })?;
    let line: usize = caps[1].parse().map_err(|_| {
        format!("Invalid line number in reference: {ref_text:?}")
    })?;
    if line == 0 {
        return Err(format!(
            "Line number must be >= 1, got {line} in \"{ref_text}\"."
        ));
    }
    Ok(ParsedTag { line })
}

// ---------------------------------------------------------------------------
// validate_line_ref
// ---------------------------------------------------------------------------

/// Returns an error when the line reference is out of bounds for the given file.
pub fn validate_line_ref(tag: &ParsedTag, file_lines: &[String]) -> Result<(), String> {
    if tag.line == 0 || tag.line > file_lines.len() {
        Err(format!(
            "Line {} does not exist (file has {} lines)",
            tag.line,
            file_lines.len()
        ))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// format_anchored_context
// ---------------------------------------------------------------------------

/// Numbered `LINE:TEXT` rows around `anchor_lines` (±[`MISMATCH_CONTEXT`]),
/// `*`-marking anchors, `...` between non-adjacent runs. Out-of-range anchors
/// contribute no rows.
pub fn format_anchored_context(anchor_lines: &[usize], file_lines: &[String]) -> Vec<String> {
    let mut display_set = std::collections::BTreeSet::new();
    for &line in anchor_lines {
        if line == 0 || line > file_lines.len() {
            continue;
        }
        let lo = 1.max(line.saturating_sub(MISMATCH_CONTEXT));
        let hi = file_lines.len().min(line + MISMATCH_CONTEXT);
        for line_num in lo..=hi {
            display_set.insert(line_num);
        }
    }

    let anchor_set: std::collections::BTreeSet<usize> = anchor_lines.iter().copied().collect();
    let mut rows: Vec<String> = Vec::new();
    let mut previous: Option<usize> = None;

    for line_num in display_set {
        if let Some(prev) = previous {
            if line_num > prev + 1 {
                rows.push("...".to_string());
            }
        }
        previous = Some(line_num);

        let marker = if anchor_set.contains(&line_num) {
            "*"
        } else {
            " "
        };
        let line_text = file_lines
            .get(line_num.wrapping_sub(1))
            .map(|s| s.as_str())
            .unwrap_or("");
        rows.push(format!(
            "{}{}",
            marker,
            format_numbered_line(line_num, line_text)
        ));
    }

    rows
}

// ---------------------------------------------------------------------------
// rejection_header
// ---------------------------------------------------------------------------

/// Build the rejection header lines shown when a hash mismatch is detected.
fn rejection_header(
    path: Option<&str>,
    expected_hash: &str,
    actual_hash: &str,
    hash_recognized: bool,
) -> Vec<String> {
    let path_text = path.map_or_else(String::new, |p| format!(" for {p}"));

    if !hash_recognized {
        vec![
            format!(
                "Edit rejected{path_text}: hash {HL_FILE_HASH_SEP}{expected_hash} \
                 is not from this session."
            ),
            format!(
                "The current file hashes to {HL_FILE_HASH_SEP}{actual_hash}. \
                 Re-read the file with `read` to copy a current \
                 {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}tag{HL_FILE_SUFFIX} header \
                 \u{2014} never invent the tag and never reuse one from a prior session."
            ),
        ]
    } else {
        vec![
            format!(
                "Edit rejected{path_text}: file changed between read and edit."
            ),
            format!(
                "Section is bound to {HL_FILE_HASH_SEP}{expected_hash}, \
                 but the current file hashes to {HL_FILE_HASH_SEP}{actual_hash}. \
                 If a prior edit in this session modified this file, copy the \
                 {HL_FILE_PREFIX}path{HL_FILE_HASH_SEP}newhash{HL_FILE_SUFFIX} header \
                 from that edit's response; otherwise re-read the file with `read` \
                 to refresh the tag before retrying."
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// format_mismatch_message
// ---------------------------------------------------------------------------

/// Build the full mismatch display message (rejection header + anchored context).
fn format_mismatch_message(
    path: Option<&str>,
    expected_hash: &str,
    actual_hash: &str,
    file_lines: &[String],
    anchor_lines: &[usize],
    hash_recognized: bool,
) -> String {
    let mut parts = rejection_header(path, expected_hash, actual_hash, hash_recognized);
    let context = format_anchored_context(anchor_lines, file_lines);
    if !context.is_empty() {
        parts.push(String::new());
        parts.extend(context);
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// MismatchDetails
// ---------------------------------------------------------------------------

/// Payload for constructing a [`MismatchError`].
pub struct MismatchDetails {
    pub path: Option<String>,
    pub expected_file_hash: String,
    pub actual_file_hash: String,
    pub file_lines: Vec<String>,
    pub anchor_lines: Vec<usize>,
    /// `true` when the section's expected hash resolved to a recorded snapshot
    /// (file content drifted since that snapshot), `false` when no snapshot
    /// was ever recorded for the hash (likely fabricated or carried over from
    /// a prior session). Defaults to `true` for backward compatibility.
    pub hash_recognized: Option<bool>,
}

// ---------------------------------------------------------------------------
// MismatchError
// ---------------------------------------------------------------------------

/// Raised when a hashline section's snapshot tag does not match the live file's
/// content (and recovery, if configured, declined the merge). Carries the file
/// lines plus anchored lines so renderers can produce a richer diagnostic
/// via [`Display`].
pub struct MismatchError {
    pub path: Option<String>,
    pub expected_hash: String,
    pub actual_hash: String,
    pub file_lines: Vec<String>,
    pub anchor_lines: Vec<usize>,
    pub hash_recognized: bool,
}

impl fmt::Display for MismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            format_mismatch_message(
                self.path.as_deref(),
                &self.expected_hash,
                &self.actual_hash,
                &self.file_lines,
                &self.anchor_lines,
                self.hash_recognized,
            )
        )
    }
}

impl fmt::Debug for MismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MismatchError")
            .field("path", &self.path)
            .field("expected_hash", &self.expected_hash)
            .field("actual_hash", &self.actual_hash)
            .field("file_lines_len", &self.file_lines.len())
            .field("anchor_lines", &self.anchor_lines)
            .field("hash_recognized", &self.hash_recognized)
            .finish()
    }
}

impl std::error::Error for MismatchError {}

impl MismatchError {
    /// Construct a new [`MismatchError`] from its detail fields.
    ///
    /// The `hash_recognized` field in `details` defaults to `true` when absent.
    pub fn new(details: MismatchDetails) -> Self {
        Self {
            path: details.path,
            expected_hash: details.expected_file_hash,
            actual_hash: details.actual_file_hash,
            file_lines: details.file_lines,
            anchor_lines: details.anchor_lines,
            hash_recognized: details.hash_recognized.unwrap_or(true),
        }
    }

    /// The user-facing display message (identical to what [`Display`] produces).
    /// Convenience accessor so callers can produce the string without going
    /// through `format!("{err}")`.
    pub fn display_message(&self) -> String {
        self.to_string()
    }
}
