use crate::search::decompose::{decompose_regex, DecomposedPattern};
use crate::search::types::{Trigram, TrigramIndex};

pub struct CandidateFilter<'a> {
    index: &'a TrigramIndex,
    pattern: &'a DecomposedPattern,
}

impl<'a> CandidateFilter<'a> {
    pub fn new(index: &'a TrigramIndex, pattern: &'a DecomposedPattern) -> Self {
        Self { index, pattern }
    }

    pub fn filter(&self) -> Vec<u32> {
        if self.pattern.is_match_all {
            return (0..self.index.line_count as u32).collect();
        }

        let required = &self.pattern.required_trigrams;
        if required.is_empty() {
            return (0..self.index.line_count as u32).collect();
        }

        let mut candidates: Vec<u32> = Vec::new();

        let first_trigram = &required[0];
        let first_postings = match self.index.get(*first_trigram) {
            Some(p) => p,
            None => return candidates,
        };

        for posting in first_postings {
            let line_idx = posting.line_idx;
            if self.line_matches_all(line_idx, &required[1..]) {
                candidates.push(line_idx);
            }
        }

        candidates
    }

    fn line_matches_all(&self, line_idx: u32, trigrams: &[Trigram]) -> bool {
        for &trigram in trigrams {
            if !self.line_has_trigram(line_idx, trigram) {
                return false;
            }
        }
        true
    }

    fn line_has_trigram(&self, line_idx: u32, trigram: Trigram) -> bool {
        if let Some(postings) = self.index.get(trigram) {
            for posting in postings {
                if posting.line_idx == line_idx {
                    return true;
                }
            }
        }
        false
    }
}

pub fn filter_candidates(index: &TrigramIndex, pattern: &str) -> (Vec<u32>, bool) {
    let decomposed = decompose_regex(pattern);

    if decomposed.is_match_all {
        return ((0..index.line_count as u32).collect(), true);
    }

    let candidates = CandidateFilter::new(index, &decomposed).filter();

    // If candidates are more than 50% of total lines, fall back to linear scan
    // Linear scan is faster when trigram filtering can't narrow down enough
    if candidates.len() >= (index.line_count as usize / 2) {
        return ((0..index.line_count as u32).collect(), true);
    }

    (candidates, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_returns_match_all_for_simple_pattern() {
        let mut index = TrigramIndex::new();
        index.set_line_count(5);

        let result = filter_candidates(&index, ".*");
        assert!(result.1);
        assert_eq!(result.0.len(), 5);
    }
}
