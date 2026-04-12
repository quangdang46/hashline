#![allow(dead_code)]

use xxhash_rust::xxh32::xxh32;

pub type ShortHash = u8;

pub fn full_hash(line: &str) -> u32 {
    full_hash_bytes(line.as_bytes())
}

pub fn full_hash_bytes(bytes: &[u8]) -> u32 {
    xxh32(bytes, 0)
}

pub fn short_hash(line: &str) -> String {
    format_short_hash(short_hash_value(line))
}

pub fn short_hash_value(line: &str) -> ShortHash {
    short_from_full(full_hash(line))
}

pub fn short_from_full(full: u32) -> ShortHash {
    (full & 0xff) as ShortHash
}

pub fn format_short_hash(short: ShortHash) -> String {
    format!("{short:02x}")
}

pub fn collides(a: &str, b: &str) -> bool {
    short_hash_value(a) == short_hash_value(b)
}

#[cfg(test)]
mod tests {
    use super::{
        collides, format_short_hash, full_hash, full_hash_bytes, short_from_full, short_hash,
        short_hash_value,
    };
    use std::collections::HashMap;
    use xxhash_rust::xxh32::xxh32;

    #[test]
    fn test_empty_line_stable() {
        assert_eq!(short_hash(""), short_hash(""));
    }

    #[test]
    fn test_whitespace_only_stable() {
        assert_eq!(short_hash("  "), short_hash("  "));
        assert_eq!(short_hash("\t"), short_hash("\t"));
    }

    #[test]
    fn test_leading_space_differs_from_no_space() {
        assert_ne!(short_hash("  return decoded"), short_hash("return decoded"));
    }

    #[test]
    fn test_trailing_space_differs_from_no_space() {
        assert_ne!(short_hash("return decoded "), short_hash("return decoded"));
    }

    #[test]
    fn test_short_hash_always_2_chars() {
        assert_eq!(short_hash("demo").len(), 2);
    }

    #[test]
    fn test_short_hash_always_lowercase_hex() {
        let hash = short_hash("demo");
        assert!(
            hash.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
    }

    #[test]
    fn test_deterministic_across_calls() {
        let first = short_hash("same line");
        let second = short_hash("same line");
        assert_eq!(first, second);
    }

    #[test]
    fn test_crlf_content_stripped_before_hashing() {
        assert_ne!(short_hash("line"), short_hash("line\r\n"));
    }

    #[test]
    fn test_collides_returns_true_on_collision() {
        let (left, right) = find_collision_pair();
        assert!(collides(&left, &right));
    }

    #[test]
    fn test_collides_returns_false_on_distinct() {
        let (left, right) = find_distinct_pair();
        assert!(!collides(&left, &right));
    }

    #[test]
    fn test_full_hash_seed_zero() {
        assert_eq!(full_hash("abc"), xxh32(b"abc", 0));
    }

    #[test]
    fn test_full_hash_bytes_seed_zero() {
        assert_eq!(full_hash_bytes(b"abc\ndef"), xxh32(b"abc\ndef", 0));
    }

    #[test]
    fn test_short_from_full_matches_short_hash() {
        let line = "alpha beta gamma";
        assert_eq!(
            format_short_hash(short_from_full(full_hash(line))),
            short_hash(line)
        );
    }

    #[test]
    fn test_numeric_short_hash_matches_string_format() {
        let line = "alpha beta gamma";
        assert_eq!(format_short_hash(short_hash_value(line)), short_hash(line));
    }

    fn find_collision_pair() -> (String, String) {
        let mut seen: HashMap<_, String> = HashMap::new();
        for i in 0..10_000 {
            let candidate = format!("line-{i}");
            let hash = short_hash_value(&candidate);
            if let Some(existing) = seen.insert(hash, candidate.clone()) {
                if existing != candidate {
                    return (existing, candidate);
                }
            }
        }
        panic!("failed to find a short-hash collision in search space");
    }

    fn find_distinct_pair() -> (String, String) {
        for i in 0..1_000 {
            let left = format!("left-{i}");
            let right = format!("right-{i}");
            if short_hash_value(&left) != short_hash_value(&right) {
                return (left, right);
            }
        }
        panic!("failed to find distinct short hashes in search space");
    }
}
