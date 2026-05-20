use regex::bytes;

use super::trigram::{Trigram, TrigramIndex};
use memchr::memchr;

/// Classification of a search pattern for optimal dispatch.
#[derive(Debug, Clone)]
pub enum PatternType {
    /// Single byte search via SIMD memchr.
    SingleByte(u8),
    /// Literal string search via token index.
    Literal(String),
    /// Multiple literals via Aho-Corasick.
    MultiLiteral(Vec<String>),
    /// Regex requiring trigram pre-filter + verify.
    Regex { case_insensitive: bool },
}

/// Classify a search pattern to determine the fastest search strategy.
pub fn classify_pattern(pattern: &str) -> PatternType {
    if pattern.len() == 1 {
        return PatternType::SingleByte(pattern.as_bytes()[0]);
    }

    if !contains_metacharacter(pattern) {
        return PatternType::Literal(pattern.to_string());
    }

    // Check if it's a pure alternation of literals: foo|bar|baz
    if let Some(literals) = extract_literal_alternatives(pattern) {
        if literals.len() > 1 {
            return PatternType::MultiLiteral(literals);
        }
    }

    // Fall back to regex (case-sensitive or case-insensitive handled at call site)
    let case_insensitive = pattern.chars().any(|c| c.eq_ignore_ascii_case(&c));
    PatternType::Regex { case_insensitive }
}

/// Returns true if the pattern contains regex metacharacters.
fn contains_metacharacter(s: &str) -> bool {
    s.bytes().any(is_metacharacter_byte)
}

fn is_metacharacter_byte(b: u8) -> bool {
    matches!(
        b,
        b'*' | b'+' | b'?' | b'.' | b'[' | b'(' | b'{' | b'^' | b'$' | b'|' | b'\\'
    )
}

/// Attempt to extract a list of literal alternatives from a pattern like "foo|bar|baz".
/// Returns None if the pattern cannot be decomposed into pure literals.
fn extract_literal_alternatives(pattern: &str) -> Option<Vec<String>> {
    let alternatives: Vec<&str> = pattern.split('|').collect();
    if alternatives.len() < 2 {
        return None;
    }
    for alt in &alternatives {
        if contains_metacharacter(alt) {
            return None;
        }
    }
    Some(alternatives.into_iter().map(String::from).collect())
}

/// Adaptive search results.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub line_idx: usize,
    pub line_text: String,
}

/// Perform adaptive search over a document.
///
/// The `line_offsets` slice gives byte offsets for each line start.
/// The `lines` slice gives the raw content string.
pub fn search_adaptive(
    pattern: &str,
    case_insensitive: bool,
    line_offsets: &[usize],
    content: &str,
) -> Vec<SearchResult> {
    let adjusted_pattern: std::borrow::Cow<'_, str> = if case_insensitive {
        std::borrow::Cow::Owned(pattern.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(pattern)
    };

    // Only allocate a lowercased copy when we actually need case folding.
    // The case-sensitive path can borrow the caller's content unchanged.
    let searched_content: std::borrow::Cow<'_, str> = if case_insensitive {
        std::borrow::Cow::Owned(content.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(content)
    };

    match classify_pattern(&adjusted_pattern) {
        PatternType::SingleByte(b) => search_single_byte(b, line_offsets, &searched_content),
        PatternType::Literal(lit) => search_literal(&lit, line_offsets, &searched_content),
        PatternType::MultiLiteral(lits) => {
            search_multi_literal(&lits, line_offsets, &searched_content)
        }
        PatternType::Regex {
            case_insensitive: _,
        } => search_regex_fallback(&adjusted_pattern, line_offsets, &searched_content),
    }
}

/// Search for a single byte using SIMD (memchr).
fn search_single_byte(b: u8, line_offsets: &[usize], content: &str) -> Vec<SearchResult> {
    let bytes = content.as_bytes();
    let mut results = Vec::new();

    for (i, windows) in line_offsets.windows(2).enumerate() {
        let start = windows[0];
        let end = windows[1].min(content.len());
        if memchr(b, &bytes[start..end]).is_some() {
            results.push(SearchResult {
                line_idx: i,
                line_text: content[start..end]
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            });
        }
    }

    results
}

/// Search for a literal using SIMD-accelerated `memchr::memmem`.
///
/// The previous implementation called `line_bytes.windows(pat_len).any(..)`,
/// a naive O(line_len * pat_len) compare. Switching to `memmem::Finder`
/// gives us the same SIMD `memchr` path ripgrep uses for short literals.
fn search_literal(lit: &str, line_offsets: &[usize], content: &str) -> Vec<SearchResult> {
    let bytes = content.as_bytes();
    let lit_bytes = lit.as_bytes();
    let pat_len = lit_bytes.len();
    let mut results = Vec::new();

    if pat_len == 0 {
        return results;
    }

    let finder = memchr::memmem::Finder::new(lit_bytes);

    for (i, windows) in line_offsets.windows(2).enumerate() {
        let start = windows[0];
        let end = windows[1].min(content.len());
        let line_bytes = &bytes[start..end];

        if pat_len > line_bytes.len() {
            continue;
        }
        if finder.find(line_bytes).is_some() {
            results.push(SearchResult {
                line_idx: i,
                line_text: content[start..end]
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            });
        }
    }

    results
}

/// Search for multiple literals using Aho-Corasick.
fn search_multi_literal(
    lits: &[String],
    line_offsets: &[usize],
    content: &str,
) -> Vec<SearchResult> {
    let ac = match aho_corasick::AhoCorasick::new(lits) {
        Ok(ac) => ac,
        Err(_) => return Vec::new(),
    };
    let bytes = content.as_bytes();
    let mut results = Vec::new();

    for (i, windows) in line_offsets.windows(2).enumerate() {
        let start = windows[0];
        let end = windows[1].min(content.len());
        if ac.find(&bytes[start..end]).is_some() {
            results.push(SearchResult {
                line_idx: i,
                line_text: content[start..end]
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            });
        }
    }

    results
}

/// Regex fallback: decompose into trigrams, find candidates, then verify with regex.
fn search_regex_fallback(
    pattern: &str,
    line_offsets: &[usize],
    content: &str,
) -> Vec<SearchResult> {
    let re = match bytes::Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // Build trigram index
    let tri_index = TrigramIndex::build_from_content(content, line_offsets);

    // Decompose regex into required trigrams
    let required: Vec<Trigram> = regex_trigrams(pattern);
    let candidates = tri_index.candidate_lines(&required);

    let bytes = content.as_bytes();
    let mut results = Vec::new();

    for line_idx in candidates.ones() {
        let start = line_offsets[line_idx];
        let end = line_offsets
            .get(line_idx + 1)
            .copied()
            .unwrap_or(content.len());
        let line_bytes = &bytes[start..end.min(content.len())];

        if re.is_match(line_bytes) {
            results.push(SearchResult {
                line_idx,
                line_text: content[start..end.min(content.len())]
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            });
        }
    }

    results
}

/// Extract up to 8 representative trigrams from a regex pattern for pre-filtering.
/// This is a simple heuristic: extract consecutive non-metachar sequences.
fn regex_trigrams(pattern_str: &str) -> Vec<Trigram> {
    let pattern = regex::escape(pattern_str);
    let mut tris = Vec::new();
    let mut chars: Vec<char> = pattern.chars().collect();

    // Sliding window of 3 consecutive non-meta chars
    let mut window = Vec::with_capacity(3);
    for c in chars.drain(..) {
        if !c.is_ascii_alphanumeric() && c != '_' {
            if window.len() == 3 {
                if let Some(tri) = chars_to_trigram(&window) {
                    tris.push(tri);
                }
                window.remove(0);
            }
            continue;
        }
        window.push(c);
        if window.len() == 3 {
            if let Some(tri) = chars_to_trigram(&window) {
                tris.push(tri);
            }
            window.remove(0);
        }
    }
    if window.len() == 3 {
        if let Some(tri) = chars_to_trigram(&window) {
            tris.push(tri);
        }
    }

    // Deduplicate
    tris.sort_by_key(|t| t.0);
    tris.dedup_by_key(|t| t.0);
    tris.truncate(8);
    tris
}

fn chars_to_trigram(chars: &[char]) -> Option<Trigram> {
    if chars.len() != 3 {
        return None;
    }
    let a = chars[0] as u8;
    let b = chars[1] as u8;
    let c = chars[2] as u8;
    Some(Trigram::new(a, b, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_single_byte() {
        assert!(matches!(
            classify_pattern("a"),
            PatternType::SingleByte(b'a')
        ));
    }

    #[test]
    fn test_classify_literal() {
        assert!(matches!(classify_pattern("hello"), PatternType::Literal(ref s) if s == "hello"));
    }

    #[test]
    fn test_classify_multi_literal() {
        let result = classify_pattern("foo|bar|baz");
        assert!(matches!(result, PatternType::MultiLiteral(ref v) if v.len() == 3));
    }

    #[test]
    fn test_classify_regex() {
        let result = classify_pattern("hel+o");
        assert!(matches!(result, PatternType::Regex { .. }));
    }

    #[test]
    fn test_search_single_byte() {
        let content = "hello\nworld\nfoo\n";
        let offsets = vec![0, 6, 12, 16];
        let results = search_single_byte(b'o', &offsets, content);
        assert_eq!(results.len(), 3); // "hello", "world", and "foo" all contain "o"
    }

    #[test]
    fn test_search_literal() {
        let content = "fn main() {\n  hello world\n}";
        let offsets = vec![0, 14, 27];
        let results = search_literal("hello", &offsets, content);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_idx, 1);
    }

    #[test]
    fn test_search_multi_literal() {
        let content = "foo bar\nbaz qux\nfoo qux\n";
        let offsets = vec![0, 8, 17, 26];
        let results =
            search_multi_literal(&["foo".to_string(), "bar".to_string()], &offsets, content);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_idx, 0);
    }

    #[test]
    fn test_search_adaptive_literal() {
        let content = "fn main() {\n  let x = 42;\n}";
        let offsets = vec![0, 14, 27];
        let results = search_adaptive("let", false, &offsets, content);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_idx, 1);
    }

    #[test]
    fn test_search_adaptive_case_insensitive() {
        let content = "Hello World\nhello world\n";
        let offsets = vec![0, 12, 24];
        let results = search_adaptive("HELLO", true, &offsets, content);
        assert_eq!(results.len(), 2);
    }
}
