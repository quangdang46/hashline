use std::collections::HashMap;
use std::sync::Arc;

use fixedbitset::FixedBitSet;

/// A bitset of line indices backed by fixedbitset.
pub type LineBitSet = FixedBitSet;

/// Token-level inverted index: maps a token (word) to the set of lines containing it.
#[derive(Debug)]
pub struct TokenIndex {
    pub(crate) tokens: HashMap<Box<str>, Arc<LineBitSet>>,
    pub(crate) line_count: usize,
}

impl TokenIndex {
    pub(crate) fn from_map(tokens: HashMap<Box<str>, Arc<LineBitSet>>, line_count: usize) -> Self {
        Self { tokens, line_count }
    }

    pub fn build(content: &str, line_offsets: &[usize]) -> Self {
        let line_count = line_offsets.len().saturating_sub(1).max(1);
        let mut tokens: HashMap<Box<str>, Arc<LineBitSet>> = HashMap::new();

        for (line_idx, windows) in line_offsets.windows(2).enumerate() {
            let start = windows[0];
            let end = windows[1].min(content.len());
            let line = &content[start..end];

            for token in tokenize_line(line.as_bytes()) {
                // Clone token for the entry key; or_insert doesn't consume key on occupied
                let bs = tokens
                    .entry(token.clone())
                    .or_insert_with(|| Arc::new(LineBitSet::with_capacity(line_count)));
                // Set bit via interior mutability
                Arc::make_mut(bs).set(line_idx, true);
            }
        }

        Self { tokens, line_count }
    }

    pub fn lookup(&self, token: &str) -> Option<Arc<LineBitSet>> {
        self.tokens.get(token).cloned()
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn bitwise_and(&self, sets: &[Arc<LineBitSet>]) -> LineBitSet {
        if sets.is_empty() {
            return LineBitSet::with_capacity(self.line_count);
        }
        let mut result = (*sets[0]).clone();
        for set in &sets[1..] {
            result.intersect_with(set.as_ref());
        }
        result
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Box<str>, &Arc<LineBitSet>)> {
        self.tokens.iter()
    }
}

/// Extract lowercase tokens from a line, stripping punctuation.
fn tokenize_line(line: &[u8]) -> Vec<Box<str>> {
    let mut tokens = Vec::new();
    let mut start = 0;

    for (i, &b) in line.iter().enumerate() {
        if is_token_boundary(b) {
            if start < i {
                let token = extract_token(&line[start..i]);
                if !token.is_empty() {
                    tokens.push(token);
                }
            }
            start = i + 1;
        }
    }

    if start < line.len() {
        let token = extract_token(&line[start..]);
        if !token.is_empty() {
            tokens.push(token);
        }
    }

    tokens
}

fn is_token_boundary(b: u8) -> bool {
    b.is_ascii_whitespace()
        || matches!(
            b,
            b'(' | b')'
                | b'{'
                | b'}'
                | b'['
                | b']'
                | b','
                | b';'
                | b':'
                | b'"'
                | b'\''
                | b'<'
                | b'>'
                | b'='
                | b'+'
                | b'-'
                | b'*'
                | b'/'
                | b'\\'
                | b'|'
                | b'&'
                | b'^'
                | b'%'
                | b'!'
                | b'@'
                | b'#'
                | b'`'
                | b'~'
        )
}

fn extract_token(slice: &[u8]) -> Box<str> {
    let trimmed = trim_bytes_punctuation(slice);
    match std::str::from_utf8(trimmed) {
        Ok(s) => s.to_ascii_lowercase().into_boxed_str(),
        Err(_) => Box::from(""),
    }
}

fn trim_bytes_punctuation(slice: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < slice.len() && slice[start].is_ascii_punctuation() && slice[start] != b'_' {
        start += 1;
    }
    let mut end = slice.len();
    while end > start && slice[end - 1].is_ascii_punctuation() && slice[end - 1] != b'_' {
        end -= 1;
    }
    &slice[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_line() {
        let tokens = tokenize_line(b"fn main() {");
        let token_strs: Vec<&str> = tokens.iter().map(|t| t.as_ref()).collect();
        assert!(token_strs.contains(&"fn"));
        assert!(token_strs.contains(&"main"));
    }

    #[test]
    fn test_token_index_build() {
        let content = "fn main() {\n  hello world\n}";
        let offsets = vec![0, 12, 25];
        let index = TokenIndex::build(content, &offsets);
        assert!(index.token_count() > 0);
        assert!(index.lookup("fn").is_some());
        assert!(index.lookup("hello").is_some());
    }

    #[test]
    fn test_bitwise_and() {
        let content = "a b c\na b d\n";
        let offsets = vec![0, 6, 13];
        let index = TokenIndex::build(content, &offsets);
        let a = index.lookup("a").unwrap();
        let b = index.lookup("b").unwrap();
        let result = index.bitwise_and(&[a, b]);
        let ones: Vec<_> = result.ones().collect();
        assert_eq!(ones, vec![0, 1]);
    }
}
