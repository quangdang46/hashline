#![allow(unused)]

use regex::Regex;
use std::sync::Arc;

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
    lines: &[Arc<str>],
    pattern: &str,
    case_insensitive: bool,
) -> Vec<VerifyResult> {
    let use_fast_path = !case_insensitive && !contains_regex_metacharacters(pattern);

    if use_fast_path {
        return verify_candidates_fast(candidates, lines, pattern);
    }

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

        let content = &lines[line_idx_usize];

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
                content: content.clone(),
                matches,
            });
        }
    }

    results
}

fn verify_candidates_fast(
    candidates: &[u32],
    lines: &[Arc<str>],
    pattern: &str,
) -> Vec<VerifyResult> {
    let mut results = Vec::new();

    for &line_idx in candidates {
        let line_idx_usize = line_idx as usize;
        if line_idx_usize >= lines.len() {
            continue;
        }

        let content = &lines[line_idx_usize];

        if let Some(pos) = content.find(pattern) {
            results.push(VerifyResult {
                line_idx,
                content: content.clone(),
                matches: vec![MatchRange {
                    start: pos,
                    end: pos + pattern.len(),
                }],
            });
        }
    }

    results
}

fn contains_regex_metacharacters(s: &str) -> bool {
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '|'
                | '\\'
                | '"'
        ) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_simple_match() {
        let lines: Vec<Arc<str>> = vec![
            Arc::from("hello world"),
            Arc::from("foo bar"),
            Arc::from("baz qux"),
        ];

        let results = verify_candidates(&[0, 1, 2], &lines, "foo", false);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_idx, 1);
    }

    #[test]
    fn test_verify_no_match() {
        let lines: Vec<Arc<str>> = vec![Arc::from("hello world"), Arc::from("foo bar")];

        let results = verify_candidates(&[0, 1], &lines, "xyz", false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_verify_case_insensitive() {
        let lines: Vec<Arc<str>> = vec![Arc::from("Hello World"), Arc::from("foo bar")];

        let results = verify_candidates(&[0, 1], &lines, "hello", true);
        assert_eq!(results.len(), 1);
    }
}
