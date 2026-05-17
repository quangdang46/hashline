#![allow(unused)]

//! Trigram extraction from text content.

use crate::search::types::{LocMask, NextMask, Posting, Trigram};

fn pack_trigram(a: u8, b: u8, c: u8) -> Trigram {
    (a as Trigram) | ((b as Trigram) << 8) | ((c as Trigram) << 16)
}

/// Extract all trigrams from a line of text.
///
/// Returns an iterator of (trigram, position, following_char) tuples.
/// The position is the byte offset within the line, and following_char
/// is the byte that follows the trigram (if any).
#[inline]
pub fn extract_trigrams(line: &[u8]) -> TrigramIter<'_> {
    TrigramIter { data: line, pos: 0 }
}

pub struct TrigramIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Iterator for TrigramIter<'_> {
    type Item = (Trigram, u8, Option<u8>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 3 > self.data.len() {
            return None;
        }

        let trigram = (self.data[self.pos] as u32)
            | ((self.data[self.pos + 1] as u32) << 8)
            | ((self.data[self.pos + 2] as u32) << 16);

        let following = if self.pos + 3 < self.data.len() {
            Some(self.data[self.pos + 3])
        } else {
            None
        };

        let result = (trigram, self.pos as u8, following);
        self.pos += 1;
        Some(result)
    }
}

/// Build postings from trigram extraction results for a single line.
pub fn build_postings_for_line(line_idx: u32, line: &[u8]) -> Vec<(Trigram, Posting)> {
    let mut postings: Vec<(Trigram, Posting)> = Vec::new();
    let mut seen: std::collections::HashMap<Trigram, (LocMask, NextMask)> =
        std::collections::HashMap::new();

    for (trigram, pos, following) in extract_trigrams(line) {
        let entry = seen
            .entry(trigram)
            .or_insert_with(|| (LocMask::default(), NextMask::default()));

        entry.0 = entry.0.union(LocMask::new(pos));
        if let Some(c) = following {
            entry.1.insert(c);
        }
    }

    for (trigram, (loc_mask, next_mask)) in seen {
        postings.push((
            trigram,
            Posting {
                line_idx,
                loc_mask,
                next_mask,
            },
        ));
    }

    postings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_trigrams_basic() {
        let line = b"abcdef";
        let trigrams: Vec<_> = extract_trigrams(line).collect();

        assert_eq!(line.len(), 6);
        assert_eq!(trigrams.len(), 4);

        // "abc" at pos 0, followed by 'd'
        assert_eq!(trigrams[0].0, pack_trigram(b'a', b'b', b'c'));
        assert_eq!(trigrams[0].1, 0);
        assert_eq!(trigrams[0].2, Some(b'd'));

        // "bcd" at pos 1, followed by 'e'
        assert_eq!(trigrams[1].0, pack_trigram(b'b', b'c', b'd'));
        assert_eq!(trigrams[1].1, 1);
        assert_eq!(trigrams[1].2, Some(b'e'));

        // "cde" at pos 2, followed by 'f'
        assert_eq!(trigrams[2].0, pack_trigram(b'c', b'd', b'e'));
        assert_eq!(trigrams[2].1, 2);
        assert_eq!(trigrams[2].2, Some(b'f'));

        // "def" at pos 3, no following char
        assert_eq!(trigrams[3].0, pack_trigram(b'd', b'e', b'f'));
        assert_eq!(trigrams[3].1, 3);
        assert_eq!(trigrams[3].2, None);
    }

    #[test]
    fn test_extract_trigrams_short_line() {
        let line = b"hi";
        assert!(line.len() < 3);
        let trigrams: Vec<_> = extract_trigrams(line).collect();
        assert!(trigrams.is_empty());
    }

    #[test]
    fn test_extract_trigrams_exactly_three() {
        let line = b"abc";
        assert_eq!(line.len(), 3);
        let trigrams: Vec<_> = extract_trigrams(line).collect();
        assert_eq!(trigrams.len(), 1);
        // "abc" at pos 0, no following char since pos+3 = len
        assert_eq!(trigrams[0].2, None);
    }

    #[test]
    fn test_build_postings() {
        let line = b"hello";
        let postings = build_postings_for_line(0, line);

        assert_eq!(postings.len(), 3);

        let mut trigram_keys: Vec<_> = postings.iter().map(|(t, _)| *t).collect();
        trigram_keys.sort();
        assert_eq!(trigram_keys.len(), 3);
    }

    #[test]
    fn test_build_postings_repeated_trigram() {
        let line = b"lalala";
        assert_eq!(line.len(), 6);

        let postings = build_postings_for_line(0, line);

        // "lal" appears at positions 0 and 2 (bytes [0,1,2] and [2,3,4])
        assert_eq!(postings.len(), 2);

        let lal_posting = postings
            .iter()
            .find(|(t, _)| *t == pack_trigram(b'l', b'a', b'l'));
        assert!(lal_posting.is_some());

        let (_, posting) = lal_posting.unwrap();
        assert!(posting.loc_mask.contains(0));
        assert!(posting.loc_mask.contains(2));
    }
}
