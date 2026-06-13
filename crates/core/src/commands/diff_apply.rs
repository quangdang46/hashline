use std::io::{Read, Write};
use std::path::Path;

use serde::Serialize;

use crate::cli::DiffApplyCmd;
use crate::commands::common::atomic_write;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::output;

/// Receipt returned by [`apply_diff`].
#[derive(Clone, Debug, Serialize)]
pub struct DiffReceipt {
    pub applied: bool,
    pub anchor_changes: Vec<(usize, usize)>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Conflict {
    /// The 1-based line number in the diff hunk header.
    pub hunk_line: usize,
    /// Human-readable description of the conflict.
    pub reason: String,
}

/// Parse a unified diff string and extract hunks.
///
/// Returns `(old_file, new_file, hunks)` where each hunk is
/// `(old_start, old_count, new_start, new_count, lines)` with the
/// lines being the raw diff lines (including `+` / `-` / ` ` prefixes).
fn parse_unified_diff(diff: &str) -> Result<(String, String, Vec<Hunk>), HashlineError> {
    let mut old_file = String::new();
    let mut new_file = String::new();
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current_hunk_lines: Vec<String> = Vec::new();
    let mut current_old_start = 0usize;
    let mut current_old_count = 0usize;
    let mut current_new_start = 0usize;
    let mut current_new_count = 0usize;
    let mut in_hunk = false;
    for (hunk_line_num, line) in diff.lines().enumerate() {
        let hunk_line_num = hunk_line_num + 1; // 1-based line number for error reporting

        if line.starts_with("--- ") && old_file.is_empty() {
            // Extract old file path (strip the leading "a/" prefix convention)
            let raw = line.strip_prefix("--- ").unwrap_or(line);
            old_file = raw.trim().to_string();
            continue;
        }
        if line.starts_with("+++ ") && new_file.is_empty() {
            let raw = line.strip_prefix("+++ ").unwrap_or(line);
            new_file = raw.trim().to_string();
            continue;
        }
        if line.starts_with("@@") {
            // Finalize previous hunk
            if in_hunk {
                hunks.push(Hunk {
                    old_start: current_old_start,
                    old_count: current_old_count,
                    new_start: current_new_start,
                    new_count: current_new_count,
                    lines: std::mem::take(&mut current_hunk_lines),
                });
            }
            in_hunk = true;

            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            let parsed = parse_hunk_header(line);
            match parsed {
                Some((os, oc, ns, nc)) => {
                    current_old_start = os;
                    current_old_count = oc;
                    current_new_start = ns;
                    current_new_count = nc;
                }
                None => {
                    return Err(HashlineError::DiffHunkMismatch {
                        hunk_line: hunk_line_num,
                    });
                }
            }
            continue;
        }

        if in_hunk {
            // Skip empty lines that might appear in the diff
            if !line.is_empty() || !current_hunk_lines.is_empty() {
                // Only add non-empty context line inside a hunk
                if line.is_empty() && !current_hunk_lines.is_empty() {
                    // An empty line inside a hunk is a valid line context
                    current_hunk_lines.push(String::new());
                } else if !line.is_empty() {
                    current_hunk_lines.push(line.to_string());
                }
            }
        }
    }

    // Finalize last hunk
    if in_hunk {
        hunks.push(Hunk {
            old_start: current_old_start,
            old_count: current_old_count,
            new_start: current_new_start,
            new_count: current_new_count,
            lines: std::mem::take(&mut current_hunk_lines),
        });
    }

    Ok((old_file, new_file, hunks))
}

/// A single parsed hunk from a unified diff.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    /// The raw diff lines (with +, -, space prefix).
    lines: Vec<String>,
}

/// Parse a hunk header line like `@@ -1,3 +1,4 @@`.
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@")?;
    let end = rest.find("@@")?;
    let rest = rest[..end].trim();

    // Split on space: we expect two groups like "-1,3" and "+1,4"
    let mut parts = rest.split_whitespace();
    let old_part = parts.next()?;
    let new_part = parts.next()?;

    let (old_start, old_count) = parse_range_pair(old_part)?;
    let (new_start, new_count) = parse_range_pair(new_part)?;

    Some((old_start, old_count, new_start, new_count))
}

/// Parse a range spec like `-1,3` → (1, 3) or `-1` → (1, 1)
fn parse_range_pair(s: &str) -> Option<(usize, usize)> {
    let s = s.strip_prefix('-').or_else(|| s.strip_prefix('+'))?;
    if let Some((start, count)) = s.split_once(',') {
        let start: usize = start.parse().ok()?;
        let count: usize = count.parse().ok()?;
        Some((start, count))
    } else {
        let start: usize = s.parse().ok()?;
        Some((start, 1))
    }
}

/// Apply a unified diff to a file atomically.
///
/// Returns `DiffReceipt` with the results. On success (`applied: true`),
/// the file has been written. On failure, no changes are made.
pub fn apply_diff(path: &Path, diff_content: &str) -> Result<DiffReceipt, HashlineError> {
    let (_old_file, _new_file, hunks) = parse_unified_diff(diff_content)?;

    if hunks.is_empty() {
        return Ok(DiffReceipt {
            applied: false,
            anchor_changes: Vec::new(),
            conflicts: vec![Conflict {
                hunk_line: 0,
                reason: "diff contains no hunks".to_string(),
            }],
        });
    }

    // Load the current file content
    let doc = Document::load(path)?;
    let current_lines: Vec<&str> = doc.lines.iter().map(|l| l.content.as_ref()).collect();

    let mut result_lines: Vec<String> = current_lines.iter().map(|s| s.to_string()).collect();
    let mut anchor_changes: Vec<(usize, usize)> = Vec::new();
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut offset: isize = 0; // Running offset from applied hunks

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        let hunk_line_no = hunk_idx + 1;

        // Compute the actual start position in the current (possibly already-patched) buffer
        let old_start_1based = hunk.old_start;
        let old_start_0based = old_start_1based.saturating_sub(1);

        // Adjust for prior applied hunks
        let adjusted_start = if offset >= 0 {
            old_start_0based.saturating_add(offset as usize)
        } else {
            old_start_0based.saturating_sub((-offset) as usize)
        };

        if adjusted_start > result_lines.len() {
            conflicts.push(Conflict {
                hunk_line: hunk_line_no,
                reason: format!(
                    "hunk start {} is beyond file length {}",
                    adjusted_start + 1,
                    result_lines.len()
                ),
            });
            continue;
        }

        // Separate the hunk into old (removals) and new (additions) lines
        let mut old_lines: Vec<String> = Vec::new();
        let mut new_lines: Vec<String> = Vec::new();

        for hunk_line in &hunk.lines {
            if hunk_line.starts_with('-') {
                old_lines.push(hunk_line.strip_prefix('-').unwrap_or("").to_string());
            } else if hunk_line.starts_with('+') {
                new_lines.push(hunk_line.strip_prefix('+').unwrap_or("").to_string());
            } else if hunk_line.starts_with(' ') {
                let content = hunk_line.strip_prefix(' ').unwrap_or("").to_string();
                old_lines.push(content.clone());
                new_lines.push(content);
            } else if hunk_line.is_empty() {
                // An empty diff line represents an empty context/removed/added line.
                // We treat it as both old and new context.
                old_lines.push(String::new());
                new_lines.push(String::new());
            }
        }

        // The context/removed lines from the old file side
        // match against current lines starting at adjusted_start
        let old_len = old_lines.len();
        if old_len == 0 {
            // Pure addition hunk: just insert new_lines at adjusted_start
            for (i, nl) in new_lines.iter().enumerate() {
                result_lines.insert(adjusted_start + i, nl.clone());
                anchor_changes.push((adjusted_start + 1 + i, adjusted_start + 1 + i));
            }
            offset += new_lines.len() as isize;
            continue;
        }

        // Try to match old_lines against result_lines starting at adjusted_start
        let available = result_lines.len().saturating_sub(adjusted_start);
        let match_len = old_len.min(available);

        // Check if the context/removed lines match
        let mut match_ok = true;
        for i in 0..match_len {
            let expected = &old_lines[i];
            let actual = &result_lines[adjusted_start + i];
            if expected != actual {
                match_ok = false;
                // Try fuzzy match - skip blank lines mismatching
                if expected.is_empty() && actual.trim().is_empty() {
                    match_ok = true;
                }
            }
        }

        if !match_ok || match_len < old_len {
            // Try to find the old lines elsewhere in the file (fuzzy search)
            let found_pos = find_context_in_lines(&result_lines, &old_lines, adjusted_start);
            match found_pos {
                Some(pos) => {
                    // Apply replacement at found position
                    // Remove old_lines
                    for _ in 0..old_lines.len() {
                        if pos < result_lines.len() {
                            result_lines.remove(pos);
                        }
                    }
                    // Insert new_lines
                    for (i, nl) in new_lines.iter().enumerate() {
                        result_lines.insert(pos + i, nl.clone());
                        anchor_changes.push((pos + 1 + i, pos + 1 + i));
                    }
                    let delta = (new_lines.len() as isize) - (old_lines.len() as isize);
                    offset += delta;
                }
                None => {
                    conflicts.push(Conflict {
                        hunk_line: hunk_line_no,
                        reason: format!(
                            "hunk context does not match file content around line {}",
                            old_start_1based
                        ),
                    });
                }
            }
        } else {
            // Match found: apply replacement at adjusted_start
            // Remove old_lines
            for _ in 0..old_lines.len() {
                result_lines.remove(adjusted_start);
            }
            // Insert new_lines
            for (i, nl) in new_lines.iter().enumerate() {
                result_lines.insert(adjusted_start + i, nl.clone());
                anchor_changes.push((adjusted_start + 1 + i, adjusted_start + 1 + i));
            }
            let delta = (new_lines.len() as isize) - (old_lines.len() as isize);
            offset += delta;
        }
    }

    if !conflicts.is_empty() {
        return Ok(DiffReceipt {
            applied: false,
            anchor_changes,
            conflicts,
        });
    }

    // Write the result
    let output = result_lines.join(
        match doc.newline {
            crate::document::NewlineStyle::Lf => "\n",
            crate::document::NewlineStyle::Crlf => "\r\n",
        },
    );

    // Preserve trailing newline behavior
    let output = if doc.trailing_newline {
        format!("{}{}", output, match doc.newline {
            crate::document::NewlineStyle::Lf => "\n",
            crate::document::NewlineStyle::Crlf => "\r\n",
        })
    } else {
        output
    };

    atomic_write(path, output.as_bytes())?;

    Ok(DiffReceipt {
        applied: true,
        anchor_changes,
        conflicts: Vec::new(),
    })
}

/// Try to find `needle_lines` within `haystack` starting from `start_pos`.
/// Returns the starting index if found, None otherwise.
fn find_context_in_lines(
    haystack: &[String],
    needle: &[String],
    start_pos: usize,
) -> Option<usize> {
    if needle.is_empty() || start_pos >= haystack.len() {
        return None;
    }

    // Try a sliding window
    let max_start = haystack.len().saturating_sub(needle.len());
    let search_start = start_pos.min(max_start);

    for pos in search_start..=max_start {
        let mut all_match = true;
        for (i, n) in needle.iter().enumerate() {
            if pos + i >= haystack.len() {
                all_match = false;
                break;
            }
            if haystack[pos + i] != *n {
                // relaxed: allow leading/trailing whitespace differences
                if haystack[pos + i].trim() != n.trim() {
                    all_match = false;
                    break;
                }
            }
        }
        if all_match {
            return Some(pos);
        }
    }

    None
}

/// Run the apply-diff command from the CLI.
pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: DiffApplyCmd,
) -> Result<(), HashlineError> {
    let diff_content = if let Some(ref diff) = cmd.diff {
        diff.clone()
    } else {
        // Read from stdin
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    };

    let receipt = apply_diff(&cmd.file, &diff_content)?;

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            output::write_json_success(ctx, &receipt).map_err(HashlineError::from)
        }
        OutputMode::Pretty => {
            if receipt.applied {
                output::write_success_line(
                    ctx,
                    &format!(
                        "Applied diff: {} anchor changes, 0 conflicts.",
                        receipt.anchor_changes.len()
                    ),
                )
                .map_err(HashlineError::from)?;
                for (old_line, new_line) in &receipt.anchor_changes {
                    output::write_success_line(
                        ctx,
                        &format!("  line {} -> {}", old_line, new_line),
                    )
                    .map_err(HashlineError::from)?;
                }
            } else {
                output::write_success_line(
                    ctx,
                    &format!(
                        "Diff failed: {} conflict(s).",
                        receipt.conflicts.len()
                    ),
                )
                .map_err(HashlineError::from)?;
                for conflict in &receipt.conflicts {
                    output::write_success_line(
                        ctx,
                        &format!(
                            "  hunk {}: {}",
                            conflict.hunk_line, conflict.reason
                        ),
                    )
                    .map_err(HashlineError::from)?;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hunk_header_no_trailing_context() {
        assert_eq!(
            parse_hunk_header("@@ -1,3 +1,4 @@"),
            Some((1, 3, 1, 4))
        );
    }

    #[test]
    fn parse_hunk_header_with_trailing_context() {
        // Git diff format with trailing function context like "@@ -1,3 +1,4 @@ fn main()"
        assert_eq!(
            parse_hunk_header("@@ -1,3 +1,4 @@ fn main()"),
            Some((1, 3, 1, 4))
        );
    }

    #[test]
    fn parse_hunk_header_single_line() {
        // Format: "@@ -1 +1 @@" (no count, defaults to 1)
        assert_eq!(
            parse_hunk_header("@@ -1 +1 @@"),
            Some((1, 1, 1, 1))
        );
    }

    #[test]
    fn parse_hunk_header_with_multiline_trailing_context() {
        // Multiple words after @@
        assert_eq!(
            parse_hunk_header("@@ -10,6 +10,8 @@ pub fn foo(bar: &str) -> Result<()>"),
            Some((10, 6, 10, 8))
        );
    }

    #[test]
    fn parse_unified_diff_empty() {
        let result = parse_unified_diff("");
        assert!(result.is_ok());
        let (old_file, new_file, hunks) = result.unwrap();
        assert_eq!(old_file, "");
        assert_eq!(new_file, "");
        assert!(hunks.is_empty());
    }

    #[test]
    fn parse_unified_diff_single_hunk() {
        let diff = "--- a/demo.txt\n+++ b/demo.txt\n@@ -1,3 +1,4 @@\n alpha\n-beta\n gamma\n+delta\n";
        let (old_file, new_file, hunks) = parse_unified_diff(diff).unwrap();
        assert_eq!(old_file, "a/demo.txt");
        assert_eq!(new_file, "b/demo.txt");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
        assert_eq!(hunks[0].lines.len(), 4);
    }

    #[test]
    fn apply_diff_simple() {
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("demo.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").unwrap();

        let diff = "@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let receipt = apply_diff(&file_path, diff).unwrap();
        assert!(receipt.applied);
        assert_eq!(receipt.conflicts.len(), 0);

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "alpha\nBETA\ngamma\n");
    }

    #[test]
    fn apply_diff_conflict() {
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("demo.txt");
        // Content doesn't match the diff context
        fs::write(&file_path, "完全不同\n不同\n内容\n").unwrap();

        let diff = "@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let receipt = apply_diff(&file_path, diff).unwrap();
        assert!(!receipt.applied);
        assert!(!receipt.conflicts.is_empty());
    }

    #[test]
    fn apply_diff_empty_no_hunks() {
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("demo.txt");
        fs::write(&file_path, "content\n").unwrap();

        let receipt = apply_diff(&file_path, "").unwrap();
        assert!(!receipt.applied);
        assert_eq!(receipt.conflicts.len(), 1);
        assert!(receipt.conflicts[0].reason.contains("no hunks"));
    }

    #[test]
    fn apply_diff_atomic_on_conflict() {
        // When there's a conflict, the file should not be modified
        use std::fs;
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("demo.txt");
        fs::write(&file_path, "original\ncontent\n").unwrap();

        let diff = "@@ -1,2 +1,2 @@\n nonexistent\n-context\n+NEW\n";
        let receipt = apply_diff(&file_path, diff).unwrap();
        assert!(!receipt.applied);

        // File must remain unchanged
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "original\ncontent\n");
    }

    #[test]
    fn parse_hunk_header_invalid_format() {
        assert!(parse_hunk_header("not a hunk header").is_none());
        assert!(parse_hunk_header("@@ invalid @@").is_none());
        assert!(parse_hunk_header("@@@ -1,3 +1,4 @@@").is_none());
    }
}
