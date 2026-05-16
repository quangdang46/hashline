#![allow(unused)]

use regex::Regex;
use std::sync::Arc;

use crate::document::LineRecord;

pub struct VerifyResult {
    pub line_idx: u32,
    pub content: Arc<str>,
    pub matches: Vec<MatchRange>,
}

#[derive(Debug, Clone)]
pub struct MatchRange {
    pub start: usize,
    pub end: usize,
}

pub fn verify_candidates(
    candidates: &[u32],
    lines: &[LineRecord],
    pattern: &str,
    case_insensitive: bool,
) -> Vec<VerifyResult> {
    let regex_pattern = if case_insensitive {
        format!("(?i){}", pattern)
    } else {
        pattern.to_string()
    };

    let re = match Regex::new(&regex_pattern) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();

    for &line_idx in candidates {
        let line_idx_usize = line_idx as usize;
        if line_idx_usize >= lines.len() {
            continue;
        }

        let content = &lines[line_idx_usize].content;

        let matches: Vec<MatchRange> = re
            .find_iter(content)
            .map(|m| MatchRange {
                start: m.start(),
                end: m.end(),
            })
            .collect();

        if !matches.is_empty() {
            results.push(VerifyResult {
                line_idx,
                content: Arc::from(content.clone()),
                matches,
            });
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::LineRecord;

    fn make_line(idx: u32, content: &str) -> LineRecord {
        LineRecord {
            content: content.to_string(),
            short_hash: idx as u8,
        }
    }

    #[test]
    fn test_verify_simple_match() {
        let lines = vec![
            make_line(0, "hello world"),
            make_line(1, "foo bar"),
            make_line(2, "baz qux"),
        ];

        let results = verify_candidates(&[0, 1, 2], &lines, "foo", false);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_idx, 1);
    }

    #[test]
    fn test_verify_no_match() {
        let lines = vec![make_line(0, "hello world"), make_line(1, "foo bar")];

        let results = verify_candidates(&[0, 1], &lines, "xyz", false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_verify_case_insensitive() {
        let lines = vec![make_line(0, "Hello World"), make_line(1, "foo bar")];

        let results = verify_candidates(&[0, 1], &lines, "hello", true);
        assert_eq!(results.len(), 1);
    }
}
