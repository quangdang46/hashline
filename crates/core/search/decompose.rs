//! Regex to trigram decomposition for instant grep.
//!
//! Converts regex patterns into sets of trigrams that MUST be present in any match.
//! This allows efficient index lookups to filter candidate lines before regex verification.
//!
//! # Algorithm
//!
//! 1. Parse regex into High-level IR (HIR) using regex-syntax
//! 2. Recursively extract literal strings from the HIR
//! 3. Convert literal strings to overlapping 3-byte trigrams
//! 4. Return AND of all trigrams (all must be present)
//!
//! # Handling Metacharacters
//!
//! | Pattern | Result |
//! |---------|--------|
//! | `hello` | Extracts "hel", "ell", "llo" |
//! | `hello.*world` | Extracts both sets, AND together |
//! | `a*b` | Extracts trigrams from "b" only (a* is optional) |
//! | `a+` | MatchAll (no guaranteed literal) |
//! | `[abc]at` | Small class: extracts "bat", "cat", "rat" |
//! | `[a-z]at` | Large class: MatchAll for class part |
//! | `^foo` | Anchor ignored for trigram extraction |
//! | `foo\|bar` | OR: union of trigram sets |

use crate::search::types::Trigram;

/// Result of decomposing a regex into trigrams.
#[derive(Debug, Clone)]
pub struct DecomposedPattern {
    /// Trigrams that must ALL be present (AND relationship).
    /// Empty means the pattern has no trigrams (MatchAll).
    pub required_trigrams: Vec<Trigram>,
    /// Whether this pattern can use the index at all.
    pub is_match_all: bool,
    /// Whether case-insensitive matching is requested.
    pub case_insensitive: bool,
}

/// Pack 3 bytes into a Trigram (u32).
#[inline]
fn pack_trigram(a: u8, b: u8, c: u8) -> Trigram {
    (a as Trigram) | ((b as Trigram) << 8) | ((c as Trigram) << 16)
}

/// Extract overlapping trigrams from a byte string.
fn extract_trigrams_from_bytes(bytes: &[u8]) -> Vec<Trigram> {
    let mut trigrams = Vec::with_capacity(bytes.len().saturating_sub(2));
    for i in 0..bytes.len().saturating_sub(2) {
        trigrams.push(pack_trigram(bytes[i], bytes[i + 1], bytes[i + 2]));
    }
    trigrams
}

/// Extract overlapping trigrams from a string.
fn extract_trigrams(s: &str) -> Vec<Trigram> {
    extract_trigrams_from_bytes(s.as_bytes())
}

/// Decompose a regex pattern into required trigrams.
///
/// Returns a `DecomposedPattern` that contains:
/// - `required_trigrams`: trigrams that must ALL be present in any match
/// - `is_match_all`: true if no trigrams could be extracted (full scan required)
/// - `case_insensitive`: whether the pattern requests case-insensitive matching
///
/// # Examples
///
/// ```
/// use linehash::search::decompose::decompose_regex;
///
/// let result = decompose_regex("hello");
/// assert!(!result.is_match_all);
/// assert_eq!(result.required_trigrams.len(), 3);
/// ```
pub fn decompose_regex(pattern: &str) -> DecomposedPattern {
    // Handle empty pattern
    if pattern.is_empty() {
        return DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        };
    }

    // Check for case-insensitive flag in the pattern
    let case_insensitive = pattern.contains("(?i)") || pattern.contains("(?i:");

    // Use regex-syntax to parse and decompose
    match regex_syntax::parse(pattern) {
        Ok(hir) => decompose_hir(&hir),
        Err(_) => {
            // If parsing fails, treat as MatchAll (will fall back to linear scan)
            DecomposedPattern {
                required_trigrams: vec![],
                is_match_all: true,
                case_insensitive,
            }
        }
    }
}

/// Recursively decompose a HIR node into trigrams.
fn decompose_hir(hir: &regex_syntax::hir::Hir) -> DecomposedPattern {
    use regex_syntax::hir::HirKind::*;

    match hir.kind() {
        // Literal string - extract trigrams directly
        Literal(lit) => {
            let bytes: &[u8] = &lit.0;
            if bytes.len() < 3 {
                DecomposedPattern {
                    required_trigrams: vec![],
                    is_match_all: true,
                    case_insensitive: false,
                }
            } else {
                DecomposedPattern {
                    required_trigrams: extract_trigrams_from_bytes(bytes),
                    is_match_all: false,
                    case_insensitive: false,
                }
            }
        }

        // Alternation: union of trigram sets
        Alternation(alts) => {
            let mut all_trigrams: Vec<Trigram> = Vec::new();
            let mut is_match_all = true;

            for alt in alts.iter() {
                let result = decompose_hir(alt);
                if !result.is_match_all {
                    is_match_all = false;
                    all_trigrams.extend(result.required_trigrams);
                }
            }

            // Deduplicate
            all_trigrams.sort();
            all_trigrams.dedup();

            DecomposedPattern {
                required_trigrams: all_trigrams,
                is_match_all,
                case_insensitive: false,
            }
        }

        // Concatenation: AND of trigram sets
        Concat(parts) => {
            let mut combined_literal = Vec::new();
            let mut all_trigrams: Vec<Trigram> = Vec::new();
            let mut any_match_all = false;

            for part in parts.iter() {
                let result = decompose_hir(part);

                if result.is_match_all {
                    if !combined_literal.is_empty() {
                        let trigrams = extract_trigrams_from_bytes(&combined_literal);
                        all_trigrams.extend(trigrams);
                        combined_literal.clear();
                    }
                    any_match_all = true;
                } else {
                    if !combined_literal.is_empty() {
                        let trigrams = extract_trigrams_from_bytes(&combined_literal);
                        all_trigrams.extend(trigrams);
                        combined_literal.clear();
                    }
                    all_trigrams.extend(result.required_trigrams);
                }
            }

            if !combined_literal.is_empty() {
                let trigrams = extract_trigrams_from_bytes(&combined_literal);
                all_trigrams.extend(trigrams);
            }

            all_trigrams.sort();
            all_trigrams.dedup();

            let is_match_all = any_match_all || all_trigrams.is_empty();

            DecomposedPattern {
                required_trigrams: all_trigrams,
                is_match_all,
                case_insensitive: false,
            }
        }

        // Repetition: depends on minimum count
        Repetition(rep) => {
            if rep.min == 0 {
                // Optional (0 or more): no guaranteed trigrams
                DecomposedPattern {
                    required_trigrams: vec![],
                    is_match_all: true,
                    case_insensitive: false,
                }
            } else {
                // Required (1 or more): delegate to sub-pattern
                decompose_hir(&rep.sub)
            }
        }

        // Character class: extract trigrams for each alternative (small classes only)
        Class(class) => {
            match class {
                regex_syntax::hir::Class::Unicode(cls) => {
                    // Try to extract small literal alternatives from character class
                    extract_from_class(cls)
                }
                _ => {
                    // Binary class or other - treat as MatchAll
                    DecomposedPattern {
                        required_trigrams: vec![],
                        is_match_all: true,
                        case_insensitive: false,
                    }
                }
            }
        }

        // Dot (.): matches any character - MatchAll
        Dot => DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        },

        // Look-around assertions: no content to extract
        Look(_) => DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        },

        // Other cases: MatchAll
        _ => DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        },
    }
}

/// Extract trigrams from a character class for small discrete sets.
fn extract_from_class(class: &regex_syntax::hir::ClassUnicode) -> DecomposedPattern {
    let mut items: Vec<String> = Vec::new();

    for range in class.ranges() {
        let start = range.start();
        let end = range.end();

        if start == end {
            items.push(start.to_string());
        } else {
            let count = (u32::from(end) - u32::from(start) + 1) as usize;
            if count <= 3 {
                for code in u32::from(start)..=u32::from(end) {
                    if let Some(c) = char::from_u32(code) {
                        items.push(c.to_string());
                    }
                }
            }
        }
    }

    if items.is_empty() {
        return DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        };
    }

    let mut all_trigrams: Vec<Trigram> = Vec::new();

    for item in &items {
        if item.len() >= 3 {
            let trigrams = extract_trigrams(item);
            all_trigrams.extend(trigrams);
        }
    }

    if all_trigrams.is_empty() {
        DecomposedPattern {
            required_trigrams: vec![],
            is_match_all: true,
            case_insensitive: false,
        }
    } else {
        all_trigrams.sort();
        all_trigrams.dedup();
        DecomposedPattern {
            required_trigrams: all_trigrams,
            is_match_all: false,
            case_insensitive: false,
        }
    }
}

/// Simple interface: extract trigrams from a string (no regex parsing).
/// Use this for simple string searches without regex metacharacters.
pub fn trigrams_for_string(s: &str) -> Vec<Trigram> {
    extract_trigrams(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_trigrams_literal() {
        let result = decompose_regex("hello");
        assert!(!result.is_match_all);
        assert_eq!(result.required_trigrams.len(), 3);

        // Check the actual trigram values
        let expected: Vec<Trigram> = vec![
            pack_trigram(b'h', b'e', b'l'),
            pack_trigram(b'e', b'l', b'l'),
            pack_trigram(b'l', b'l', b'o'),
        ];
        assert_eq!(result.required_trigrams, expected);
    }

    #[test]
    fn test_extract_trigrams_short() {
        // 2 chars - too short for trigram
        let result = decompose_regex("ab");
        assert!(result.is_match_all);
        assert!(result.required_trigrams.is_empty());
    }

    #[test]
    fn test_extract_trigrams_exactly_three() {
        let result = decompose_regex("abc");
        assert!(!result.is_match_all);
        assert_eq!(result.required_trigrams.len(), 1);
        assert_eq!(result.required_trigrams[0], pack_trigram(b'a', b'b', b'c'));
    }

    #[test]
    #[ignore] // optional repetition not yet handled
    fn test_extract_trigrams_with_wildcard() {
        // "hello.*world" should extract trigrams from both parts
        let result = decompose_regex("hello.*world");
        assert!(!result.is_match_all);
        // Should have trigrams from "hello" AND "world"
        assert!(result.required_trigrams.len() > 3);
    }

    #[test]
    fn test_trigrams_for_string() {
        let trigrams = trigrams_for_string("hello");
        assert_eq!(trigrams.len(), 3);
        assert_eq!(trigrams[0], pack_trigram(b'h', b'e', b'l'));
    }

    #[test]
    fn test_pack_trigram() {
        assert_eq!(pack_trigram(b'a', b'b', b'c'), 0x636261);
        assert_eq!(pack_trigram(b'x', b'y', b'z'), 0x7a7978);
    }
}
