//! SHA-256 window hashing — backward-compat surface for callers
//! using the older anchor format.
//!
//! Native hashline anchors use [`xxh32`](crate::hash) (1-byte short
//! hash). This module provides a parallel hash function for the
//! SHA-256-over-line-range format used by some external tools
//! (notably the `hashline_edit` tool in jcode prior to 0.2.x).
//!
//! The verify + edit semantics (window slice math, error wording,
//! ambiguity rejection) match exactly so downstream callers can
//! delegate to this module without changing observable behavior.
//!
//! # Feature gate
//!
//! This module is compiled only when the `sha256-anchors` feature
//! is enabled:
//!
//! ```toml
//! [dependencies]
//! hashline = { version = "0.2", features = ["sha256-anchors"] }
//! ```
//!
//! Default builds don't pay for the `sha2` dependency.
//!
//! # Quick start
//!
//! ```no_run
//! # use hashline::sha256_window::{hash_window, verify_anchor, apply_edit_within_window};
//! let content = "fn main() {\n    println!(\"hello\");\n}\n";
//! let expected = hash_window(content, 2, 2);
//!
//! verify_anchor(content, 2, &expected, 0).unwrap();
//!
//! let (new_content, start, end) = apply_edit_within_window(
//!     content, 2,
//!     "    println!(\"hello\");",
//!     "    println!(\"world\");",
//!     0,
//! ).unwrap();
//! assert!(new_content.contains("world"));
//! ```

use crate::error::HashlineError;
use sha2::{Digest, Sha256};

/// Compute SHA-256 hash of the lines in the given range (1-indexed,
/// inclusive on both ends). Returns an empty string when the range
/// is fully out of the file.
pub fn hash_window(content: &str, start_line: usize, end_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let start = start_line.saturating_sub(1).min(total);
    let end = end_line.min(total).saturating_sub(1);

    if start > end || start >= total {
        return String::new();
    }

    let window: String = lines[start..=end].join("\n");
    let mut hasher = Sha256::new();
    hasher.update(window.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify the SHA-256 anchor hash matches the content at the given
/// line.
///
/// The anchor window is `[anchor_line - context_window,
/// anchor_line + context_window]`, clamped to the file extents.
///
/// Returns:
/// - `Err` with `HashlineError::AnchorOutOfRange` when `anchor_line`
///   is 0 or beyond the file end.
/// - `Err` with `HashlineError::AnchorDrift` when the hash mismatches.
/// - `Ok(())` on success.
pub fn verify_anchor(
    content: &str,
    anchor_line: usize,
    expected_hash: &str,
    context_window: usize,
) -> Result<(), HashlineError> {
    let total_lines = content.lines().count();
    if anchor_line == 0 || anchor_line > total_lines {
        return Err(HashlineError::Sha256Anchor(format!(
            "anchor line {anchor_line} is out of range (file has {total_lines} lines)"
        )));
    }

    let start = anchor_line.saturating_sub(context_window + 1);
    let end = (anchor_line + context_window).min(total_lines);

    let computed = hash_window(content, start + 1, end);
    if computed != expected_hash {
        return Err(HashlineError::Sha256Anchor(format!(
            "anchor drifted: file changed since plan; expected {expected_hash}, got {computed}"
        )));
    }
    Ok(())
}

/// Apply the edit within the verified anchor window only.
///
/// The anchor window is `[anchor_line - context_window,
/// anchor_line + context_window]`, clamped to the file extents.
///
/// The function:
/// 1. Extracts that window of lines.
/// 2. Asserts `old_string` appears exactly once in the window.
/// 3. Replaces that one occurrence with `new_string`.
/// 4. Returns the rewritten file content + the 1-indexed line range
///    of the replacement in the new file.
///
/// Returns `Err` if `old_string` is missing, ambiguous, or the window
/// is degenerate.
pub fn apply_edit_within_window(
    content: &str,
    anchor_line: usize,
    old_string: &str,
    new_string: &str,
    context_window: usize,
) -> Result<(String, usize, usize), HashlineError> {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return Err(HashlineError::Sha256Anchor("file is empty".to_owned()));
    }

    // 0-indexed window bounds.
    let window_start = anchor_line.saturating_sub(context_window + 1);
    let window_end = (anchor_line - 1 + context_window).min(total_lines - 1);

    if window_start > window_end {
        return Err(HashlineError::Sha256Anchor(format!(
            "anchor window out of range: lines {} to {} but file has {} lines",
            window_start + 1,
            window_end + 1,
            total_lines
        )));
    }

    let window_lines = &lines[window_start..=window_end];
    let window_text = window_lines.join("\n");

    if !window_text.contains(old_string) {
        return Err(HashlineError::Sha256Anchor(format!(
            "old_string not found within anchor window (lines {} to {}, context_window={}). \
             The anchor hash verified but old_string was not found in that region. \
             Make sure old_string exactly matches the content within the anchor window.",
            window_start + 1,
            window_end + 1,
            context_window
        )));
    }

    let occurrences = window_text.matches(old_string).count();
    if occurrences > 1 {
        return Err(HashlineError::Sha256Anchor(format!(
            "old_string found {occurrences} times within the anchor window. \
             Provide a more specific old_string or adjust context_window to narrow the search region."
        )));
    }

    // Find the global byte offset of old_string in the original content.
    let window_offset = window_text.find(old_string).unwrap();
    let global_offset = lines[..window_start]
        .iter()
        .map(|l| l.len() + 1)
        .sum::<usize>()
        + window_offset;

    let prefix = &content[..global_offset];
    let start_line = prefix.lines().count() + 1;

    let mut result = String::with_capacity(content.len());
    result.push_str(&content[..global_offset]);
    result.push_str(new_string);
    let old_end = global_offset + old_string.len();
    result.push_str(&content[old_end..]);

    Ok((
        result,
        start_line,
        start_line + new_string.lines().count().saturating_sub(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_content() -> &'static str {
        "fn main() {\n    println!(\"hello\");\n    let x = 1;\n    println!(\"x={}\", x);\n}\n"
    }

    #[test]
    fn hash_window_single_line() {
        let content = test_content();
        let h1 = hash_window(content, 1, 1);
        assert!(!h1.is_empty());
        let h2 = hash_window(content, 2, 2);
        assert!(!h2.is_empty());
        assert_ne!(h1, h2);
        assert_eq!(h1, hash_window(content, 1, 1));
    }

    #[test]
    fn hash_window_multiple_lines() {
        let content = test_content();
        let h = hash_window(content, 2, 3);
        assert!(!h.is_empty());
        assert_eq!(h, hash_window(content, 2, 3));
    }

    #[test]
    fn hash_window_out_of_range_returns_empty() {
        let content = test_content();
        assert!(hash_window(content, 100, 105).is_empty());
    }

    #[test]
    fn verify_anchor_success() {
        let content = test_content();
        let line = 2;
        let hash = hash_window(content, line, line);
        assert!(verify_anchor(content, line, &hash, 0).is_ok());
    }

    #[test]
    fn verify_anchor_with_context() {
        let content = test_content();
        let center = 2;
        let hash = hash_window(content, 1, 3);
        assert!(verify_anchor(content, center, &hash, 1).is_ok());
    }

    #[test]
    fn verify_anchor_drifted() {
        let content = test_content();
        let result = verify_anchor(content, 2, "deadbeef", 0);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("anchor drifted"), "{err}");
    }

    #[test]
    fn verify_anchor_out_of_range() {
        let content = test_content();
        let hash = hash_window(content, 1, 1);
        let result = verify_anchor(content, 99, &hash, 0);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("out of range"), "{err}");
    }

    #[test]
    fn apply_edit_success() {
        let content = test_content();
        let (new, start, _end) = apply_edit_within_window(
            content,
            2,
            "    println!(\"hello\");",
            "    println!(\"world\");",
            0,
        )
        .unwrap();
        assert!(new.contains("world"));
        assert!(!new.contains("hello"));
        assert_eq!(start, 2);
    }

    #[test]
    fn apply_edit_not_in_window() {
        let content = test_content();
        let result = apply_edit_within_window(
            content,
            5,
            "    println!(\"hello\");",
            "    println!(\"world\");",
            0,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("anchor window out of range") || err.contains("old_string not found"),
            "{err}"
        );
    }

    #[test]
    fn apply_edit_ambiguous() {
        let content = "    x = 1;\n    x = 2;\n";
        let result = apply_edit_within_window(content, 1, "    x = ", "    y = ", 1);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("found 2 times"), "{err}");
    }

    #[test]
    fn apply_edit_ctx_zero_isolates_to_anchor_line() {
        let content = "    x = 1;\n    x = 2;\n";
        let (new_content, start, end) =
            apply_edit_within_window(content, 1, "    x = ", "    y = ", 0)
                .expect("ctx=0 must operate only on the anchor line");
        assert_eq!(start, 1);
        assert_eq!(end, 1);
        let lines: Vec<&str> = new_content.lines().collect();
        assert!(lines[0].contains("y = 1"));
        assert!(lines[1].contains("x = 2"));
    }

    #[test]
    fn crlf_normalization() {
        let content = "line1\r\nline2\r\nline3\r\n";
        let h = hash_window(content, 2, 2);
        assert!(!h.is_empty());
    }

    #[test]
    fn multibyte_content() {
        let content = "fn main() {\n    println!(\"你好\");\n    let emoji = \"🎉\";\n}\n";
        let h = hash_window(content, 2, 3);
        assert!(!h.is_empty());

        let (_, start, _) = apply_edit_within_window(
            content,
            2,
            "    println!(\"你好\");",
            "    println!(\"hola\");",
            0,
        )
        .unwrap();
        assert_eq!(start, 2);
    }

    /// Regression: edits to the last line of a file used to fail with
    /// "anchor window out of range" because the 0-indexed slice end
    /// was confused with the 1-indexed line number.
    #[test]
    fn apply_edit_on_last_line() {
        let content = "first\nsecond\nlast\n";
        let total_lines = content.lines().count();
        assert_eq!(total_lines, 3);

        let (new_content, start, end) = apply_edit_within_window(content, 3, "last", "final", 0)
            .expect("editing the last line must work");

        assert!(new_content.contains("final"));
        assert!(!new_content.contains("last"));
        assert_eq!(start, 3);
        assert_eq!(end, 3);
    }

    #[test]
    fn apply_edit_on_last_line_with_context() {
        let content = "first\nsecond\nthird\n";
        let (new_content, _, _) = apply_edit_within_window(content, 3, "third", "fourth", 1)
            .expect("last line + context window must still resolve");
        assert!(new_content.contains("fourth"));
    }

    #[test]
    fn apply_edit_on_only_line() {
        let content = "only\n";
        let (new_content, start, end) = apply_edit_within_window(content, 1, "only", "changed", 0)
            .expect("single-line file must be editable");
        assert!(new_content.starts_with("changed"));
        assert_eq!(start, 1);
        assert_eq!(end, 1);
    }

    /// Hash computed for verify_anchor must match the window
    /// apply_edit_within_window operates on. Otherwise the verified
    /// region differs from the edited region — the fundamental
    /// correctness invariant of the whole tool family.
    #[test]
    fn verify_and_apply_use_consistent_window() {
        let content =
            "fn main() {\n    println!(\"a\");\n    println!(\"b\");\n    println!(\"c\");\n}\n";
        for anchor in 1usize..=5 {
            for ctx in 0usize..=2 {
                let h = {
                    let total = content.lines().count();
                    let start = anchor.saturating_sub(ctx + 1);
                    let end = (anchor + ctx).min(total);
                    hash_window(content, start + 1, end)
                };
                let v = verify_anchor(content, anchor, &h, ctx);
                assert!(
                    v.is_ok(),
                    "verify failed for anchor={anchor}, ctx={ctx}, hash={h}: {:?}",
                    v.err()
                );
            }
        }
    }
}
