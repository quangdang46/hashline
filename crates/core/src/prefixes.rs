//! Read-output line-number prefix stripping helpers.
//!
//! When a hashline payload is authored against `read`/`search` output, each
//! line is prefixed with either a hashline-mode line number (`123:`) or, for
//! diff-style echoes, a leading `+`. These helpers detect and strip them.

use crate::patch_format::HL_FILE_HASH_LENGTH;

/// Matches leading hashline line-number prefixes: optional `>>>`/`>>` whitespace,
/// optional `+`/`-`/`*`, then `digits:`
fn strip_one_leading_hashline_prefix(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut i = 0;

    // Skip leading whitespace
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // Skip optional `>>>` or `>>`
    if i + 2 < bytes.len() && bytes[i] == b'>' && bytes[i + 1] == b'>' {
        i += 2;
        if bytes[i] == b'>' {
            i += 1;
        }
    }
    // Skip optional whitespace after arrows
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // Skip optional `+`, `-`, `*`
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-' || bytes[i] == b'*') {
        i += 1;
    }
    // Skip optional whitespace after sigil
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // Parse digits followed by ':'
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > digits_start && i < bytes.len() && bytes[i] == b':' {
        return line[i + 1..].to_owned();
    }
    line.to_owned()
}

/// Matches `[path#HASH]` headers
fn is_header_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return false;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if let Some(hash_pos) = inner.rfind('#') {
        let hash_part = &inner[hash_pos + 1..];
        hash_part.len() == HL_FILE_HASH_LENGTH && hash_part.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        false
    }
}

/// Matches `[Showing lines N-M of K...]` truncation notices
fn is_truncation_notice(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("[Showing lines ") || trimmed.contains("more lines")
}

fn collect_line_prefix_stats(lines: &[String]) -> LinePrefixStats {
    let mut stats = LinePrefixStats::default();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if is_truncation_notice(line) {
            stats.truncation_notice_count += 1;
            continue;
        }
        if is_header_line(line) {
            stats.non_empty += 1;
            stats.header_count += 1;
            continue;
        }
        stats.non_empty += 1;

        let stripped = strip_one_leading_hashline_prefix(line);
        if stripped != *line {
            stats.hash_prefix_count += 1;
            // Check for `+N:` form
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() && bytes[i] == b' ' { i += 1; }
            if i < bytes.len() && bytes[i] == b'+' {
                stats.diff_plus_hash_prefix_count += 1;
            }
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with('+') && !trimmed.starts_with("++") {
            stats.diff_plus_count += 1;
        }
    }
    stats
}

#[derive(Default)]
struct LinePrefixStats {
    non_empty: usize,
    header_count: usize,
    hash_prefix_count: usize,
    diff_plus_hash_prefix_count: usize,
    diff_plus_count: usize,
    truncation_notice_count: usize,
}

/// Strip whichever prefix scheme the lines appear to be carrying:
/// - hashline line-number prefixes (`123:`) when every content line has one
/// - leading `+` (diff style) when at least half the lines have one
/// - mixed `+N:` form when present
///
/// Returns the lines untouched if no scheme is recognized.
pub fn strip_new_line_prefixes(lines: &[String]) -> Vec<String> {
    let stats = collect_line_prefix_stats(lines);
    if stats.non_empty == 0 {
        return lines.to_vec();
    }

    let content_line_count = stats.non_empty - stats.header_count;
    let strip_hash = content_line_count > 0 && stats.hash_prefix_count == content_line_count;
    let strip_plus = !strip_hash
        && stats.diff_plus_hash_prefix_count == 0
        && stats.diff_plus_count > 0
        && stats.diff_plus_count >= (stats.non_empty as f64 * 0.5) as usize;

    if !strip_hash && !strip_plus && stats.diff_plus_hash_prefix_count == 0 {
        return lines.to_vec();
    }

    lines
        .iter()
        .filter(|line| {
            !is_truncation_notice(line)
                && !(strip_hash && is_header_line(line))
        })
        .map(|line| {
            if strip_hash {
                strip_one_leading_hashline_prefix(line)
            } else if strip_plus {
                let trimmed = line.trim_start();
                if trimmed.starts_with('+') && !trimmed.starts_with("++") {
                    line.replacen('+', "", 1)
                } else {
                    line.clone()
                }
            } else {
                line.clone()
            }
        })
        .collect()
}

/// Strip a single leading hashline prefix (`N:`, `>>>N:`, `+N:` etc.).
/// Unlike the full `strip_new_line_prefixes`, this strips at most one prefix
/// and does NOT loop — safe for individually stripped rows.
pub fn strip_one_hashline_prefix(line: &str) -> String {
    strip_one_leading_hashline_prefix(line)
}

/// Normalize line payloads by stripping read/search line prefixes.
pub fn hashline_parse_text(edit: &str) -> Vec<String> {
    let trimmed = edit.strip_suffix('\n').unwrap_or(edit);
    let lines: Vec<String> = trimmed.replace("\r", "").split('\n').map(String::from).collect();
    strip_new_line_prefixes(&lines)
}
