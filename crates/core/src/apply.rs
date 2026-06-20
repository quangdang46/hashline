//! Boundary echo repair helpers for hashline replacement edits.
//!
//! Detects common LLM mistakes in `SWAP A..B:` replacement ranges:
//! - Boundary echoes (payload restates unchanged lines at range edges)
//! - Duplicate prefixes/suffixes
//! - Dropped structural closers
//!   All repairs emit warnings but never fail — conservative by default.

/// Check if a line is a structural closer (braces, parens, end keyword).
pub fn is_structural_closer(trimmed: &str) -> bool {
    trimmed == "}"
        || trimmed == "});"
        || trimmed == "},"
        || trimmed == ")"
        || trimmed == "]);"
        || trimmed == "])"
        || trimmed == "]"
        || trimmed == "end"
}

/// Given a replacement payload (lines to insert) and the range it replaces
/// (`start_line` and `end_line`, 1-indexed), detect and report any boundary issues.
///
/// Returns warnings describing detected issues. Does NOT modify the payload.
pub fn detect_boundary_issues(
    payload: &[String],
    start_line: usize,
    end_line: usize,
    entries: &[crate::document::LineEntry],
) -> Vec<String> {
    let mut warnings = Vec::new();
    if payload.is_empty() || entries.is_empty() {
        return warnings;
    }

    let entries_len = entries.len();

    // Check for boundary echo: payload's first line matches content just above range
    if start_line > 1 && start_line <= entries_len {
        let line_above = &entries[start_line - 2].content;
        if payload[0] == *line_above {
            warnings.push(
                "Boundary echo detected: first payload line duplicates the line above the range. \
                 Use SWAP on the range whose content actually changes."
                    .to_string(),
            );
        }
    }

    // Check for boundary echo: payload's last line matches content just below range
    if end_line < entries_len {
        let line_below = &entries[end_line].content;
        if payload.last().is_some_and(|p| p == line_below) {
            warnings.push(
                "Boundary echo detected: last payload line duplicates the line below the range. \
                 Use SWAP on the range whose content actually changes."
                    .to_string(),
            );
        }
    }

    // Check for dropped suffix closers: range deletes structural closers
    // that the payload doesn't restate.
    let payload_has_closer = payload.iter().any(|l| is_structural_closer(l.trim()));
    if !payload_has_closer {
        let mut deleted_closers = 0;
        for i in start_line..=end_line.min(entries_len) {
            if is_structural_closer(entries[i - 1].content.trim()) {
                deleted_closers += 1;
            }
        }
        if deleted_closers > 0 {
            warnings.push(format!(
                "Dropped closers: the range deleted {deleted_closers} structural closer(s) \
                 but the payload does not restate them. Consider widening the SWAP body \
                 or narrowing the range.",
            ));
        }
    }

    warnings
}

/// Check if a payload likely has a leading duplicate prefix (restates content above range).
/// Returns the number of leading duplicate lines, or 0.
pub fn duplicate_prefix_count(
    payload: &[String],
    start_line: usize,
    entries: &[crate::document::LineEntry],
) -> usize {
    if payload.is_empty() || start_line <= 1 {
        return 0;
    }
    let max_check = payload.len().min(start_line - 1);
    let mut count = 0;
    for i in 0..max_check {
        let line_above = &entries[start_line - 2 - i].content;
        if payload[payload.len() - 1 - i] == *line_above {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Check if a payload likely has a trailing duplicate suffix (restates content below range).
/// Returns the number of trailing duplicate lines, or 0.
pub fn duplicate_suffix_count(
    payload: &[String],
    end_line: usize,
    entries: &[crate::document::LineEntry],
) -> usize {
    if payload.is_empty() || end_line >= entries.len() {
        return 0;
    }
    let max_check = payload.len().min(entries.len() - end_line);
    let mut count = 0;
    for i in 0..max_check {
        let line_below = &entries[end_line + i].content;
        if payload[payload.len() - 1 - i] == *line_below {
            count += 1;
        } else {
            break;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::LineEntry;

    fn make_entry(content: &str) -> LineEntry {
        LineEntry {
            content: content.to_string(),
            short_hash: 0,
        }
    }

    #[test]
    fn test_is_structural_closer() {
        assert!(is_structural_closer("}"));
        assert!(is_structural_closer("});"));
        assert!(is_structural_closer("end"));
        assert!(!is_structural_closer("fn main() {"));
        assert!(!is_structural_closer("    let x = 1;"));
    }

    #[test]
    fn test_detect_boundary_echo_no_issues() {
        let payload = vec!["fn new_func() {".to_string(), "    let x = 1;".to_string()];
        let entries = vec![make_entry("fn old_func() {"), make_entry("    let x = 2;")];
        let warnings = detect_boundary_issues(&payload, 1, 1, &entries);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_detect_boundary_echo_leading() {
        let payload = vec!["fn old_func() {".to_string(), "    let x = 2;".to_string()];
        let entries = vec![make_entry("fn old_func() {"), make_entry("    let x = 2;")];
        let warnings = detect_boundary_issues(&payload, 2, 2, &entries);
        // First payload line \'fn old_func() {\' matches line above (line 1)
        assert!(warnings.iter().any(|w| w.contains("Boundary echo")));
    }

    #[test]
    fn test_detect_dropped_closers() {
        // Range 1..=3 includes the closing brace, but payload doesn't restate it
        let payload = vec!["    let x = 1;".to_string()];
        let entries = vec![
            make_entry("fn test() {"),
            make_entry("    let x = 1;"),
            make_entry("}"),
        ];
        let warnings = detect_boundary_issues(&payload, 1, 3, &entries);
        assert!(warnings.iter().any(|w| w.contains("Dropped closers")));
    }

    #[test]
    fn test_duplicate_prefix_count() {
        let payload = vec!["keep me".to_string(), "same line".to_string()];
        let entries = vec![make_entry("same line")];
        assert_eq!(duplicate_prefix_count(&payload, 2, &entries), 1);
    }

    #[test]
    fn test_duplicate_suffix_count() {
        let payload = vec!["same line".to_string()];
        let entries = vec![make_entry("keep me"), make_entry("same line")];
        assert_eq!(duplicate_suffix_count(&payload, 1, &entries), 1);
    }

    #[test]
    fn test_no_false_positive_boundary_echo() {
        let payload = vec!["different content".to_string()];
        let entries = vec![make_entry("same line")];
        let warnings = detect_boundary_issues(&payload, 2, 2, &entries);
        assert!(warnings.is_empty());
    }
}
