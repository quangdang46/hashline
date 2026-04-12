#![allow(unused)]

//! Index builder - constructs TrigramIndex from document lines.

use crate::search::extract::build_postings_for_line;
use crate::search::types::{IndexMeta, TrigramIndex};

pub struct IndexBuilder {
    index: TrigramIndex,
    line_count: usize,
}

impl IndexBuilder {
    pub fn new() -> Self {
        Self {
            index: TrigramIndex::new(),
            line_count: 0,
        }
    }

    pub fn add_line(&mut self, line_idx: usize, content: &[u8]) {
        let postings = build_postings_for_line(line_idx as u32, content);
        for (trigram, posting) in postings {
            self.index.insert(trigram, posting);
        }
        if line_idx >= self.line_count {
            self.line_count = line_idx + 1;
        }
    }

    pub fn add_lines<I>(&mut self, lines: I)
    where
        I: Iterator<Item = Vec<u8>>,
    {
        for (line_idx, line) in lines.enumerate() {
            self.add_line(line_idx, &line);
        }
    }

    pub fn build(mut self) -> TrigramIndex {
        self.index.set_line_count(self.line_count);
        self.index
    }

    pub fn build_with_meta(
        mut self,
        file_mtime: u64,
        file_size: u64,
        content_hash: u64,
    ) -> (TrigramIndex, IndexMeta) {
        self.index.set_line_count(self.line_count);
        let meta = IndexMeta {
            file_mtime,
            file_size,
            content_hash,
            line_count: self.line_count as u32,
        };
        (self.index, meta)
    }
}

impl Default for IndexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn compute_content_hash(content: &[u8]) -> u64 {
    xxhash_rust::xxh32::xxh32(content, 0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_builder_empty() {
        let builder = IndexBuilder::new();
        let index = builder.build();
        assert_eq!(index.trigram_count(), 0);
        assert_eq!(index.line_count, 0);
    }

    #[test]
    fn test_index_builder_single_line() {
        let mut builder = IndexBuilder::new();
        builder.add_line(0, b"hello");

        let index = builder.build();
        assert!(index.trigram_count() > 0);
        assert_eq!(index.line_count, 1);
    }

    #[test]
    fn test_index_builder_multiple_lines() {
        let mut builder = IndexBuilder::new();
        builder.add_line(0, b"hello");
        builder.add_line(1, b"world");

        let index = builder.build();
        assert!(index.trigram_count() > 0);
        assert_eq!(index.line_count, 2);
    }

    #[test]
    fn test_index_builder_with_lines_iterator() {
        let lines: Vec<Vec<u8>> = vec![b"foo".to_vec(), b"bar".to_vec(), b"baz".to_vec()];
        let mut builder = IndexBuilder::new();
        builder.add_lines(lines.into_iter());

        let index = builder.build();
        assert!(index.trigram_count() > 0);
        assert_eq!(index.line_count, 3);
    }

    #[test]
    fn test_content_hash() {
        let hash1 = compute_content_hash(b"hello");
        let hash2 = compute_content_hash(b"hello");
        let hash3 = compute_content_hash(b"world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
