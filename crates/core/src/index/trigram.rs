use std::collections::HashMap;
use std::sync::Arc;

use super::token::LineBitSet;

/// A trigram is 3 bytes packed into a u32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trigram(pub [u8; 3]);

impl Trigram {
    /// Create a trigram from 3 bytes.
    pub fn new(a: u8, b: u8, c: u8) -> Self {
        Self([a, b, c])
    }

    /// Extract all trigrams from a byte slice.
    pub fn from_slice(slice: &[u8]) -> Vec<Trigram> {
        slice
            .windows(3)
            .map(|w| Self::new(w[0], w[1], w[2]))
            .collect()
    }

    /// Return the raw bytes.
    pub fn as_bytes(&self) -> [u8; 3] {
        self.0
    }
}

/// Trigram-level inverted index: maps a trigram to the set of lines containing it.
#[derive(Debug)]
pub struct TrigramIndex {
    /// Public for use by the persistence layer.
    pub(crate) trigrams: HashMap<Trigram, Arc<LineBitSet>>,
    pub(crate) line_count: usize,
}

impl TrigramIndex {
    /// Construct from an existing map (used by persistence layer).
    pub(crate) fn from_map(trigrams: HashMap<Trigram, Arc<LineBitSet>>, line_count: usize) -> Self {
        Self {
            trigrams,
            line_count,
        }
    }

    /// Build from content and line offsets.
    pub fn build_from_content(content: &str, line_offsets: &[usize]) -> Self {
        let actual_count = line_offsets.len().saturating_sub(1).max(1);
        let mut trigrams: HashMap<Trigram, Arc<LineBitSet>> = HashMap::new();

        for (line_idx, windows) in line_offsets.windows(2).enumerate() {
            let start = windows[0];
            let end = windows[1].min(content.len());
            let line_bytes = &content.as_bytes()[start..end];

            for tri in Trigram::from_slice(line_bytes) {
                let bs = trigrams
                    .entry(tri)
                    .or_insert_with(|| Arc::new(LineBitSet::with_capacity(actual_count)));
                let mut bs_mut = (**bs).clone();
                bs_mut.set(line_idx, true);
                if **bs != bs_mut {
                    trigrams.insert(tri, Arc::new(bs_mut));
                }
            }
        }

        Self {
            trigrams,
            line_count: actual_count,
        }
    }

    /// Look up a trigram and return the set of lines containing it.
    pub fn lookup(&self, trigram: Trigram) -> Option<Arc<LineBitSet>> {
        self.trigrams.get(&trigram).cloned()
    }

    /// Return the number of unique trigrams.
    pub fn trigram_count(&self) -> usize {
        self.trigrams.len()
    }

    /// Return the total line count.
    pub fn line_count(&self) -> usize {
        self.line_count
    }

    /// Compute candidate lines for a multi-trigram query using AND logic.
    pub fn candidate_lines(&self, required_trigrams: &[Trigram]) -> LineBitSet {
        if required_trigrams.is_empty() {
            return LineBitSet::with_capacity(self.line_count);
        }

        let mut result = LineBitSet::with_capacity(self.line_count);
        result.set_range(.., true);

        for tri in required_trigrams {
            if let Some(set) = self.lookup(*tri) {
                result.intersect_with(set.as_ref());
            } else {
                result.clear();
                break;
            }
        }

        result
    }

    /// Iterate (for persistence).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Trigram, &Arc<LineBitSet>)> {
        self.trigrams.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigram_from_slice() {
        let tris = Trigram::from_slice(b"hello");
        assert_eq!(tris.len(), 3);
        assert_eq!(tris[0].0, [b'h', b'e', b'l']);
    }

    #[test]
    fn test_trigram_index_build() {
        let content = "hello world\nfoo bar\n";
        let offsets = vec![0, 12, 20];
        let index = TrigramIndex::build_from_content(content, &offsets);
        assert!(index.trigram_count() > 0);
        assert!(index.lookup(Trigram::new(b'e', b'l', b'l')).is_some());
        assert!(index.lookup(Trigram::new(b'x', b'y', b'z')).is_none());
    }

    #[test]
    fn test_candidate_lines() {
        let content = "hello world\nfoo bar\n";
        let offsets = vec![0, 12, 20];
        let index = TrigramIndex::build_from_content(content, &offsets);
        let tris = vec![
            Trigram::new(b'h', b'e', b'l'),
            Trigram::new(b'w', b'o', b'r'),
        ];
        let candidates = index.candidate_lines(&tris);
        let ones: Vec<_> = candidates.ones().collect();
        assert_eq!(ones, vec![0]);
    }
}
