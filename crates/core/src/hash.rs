#![allow(dead_code)]

use xxhash_rust::xxh32::xxh32;
use xxhash_rust::xxh3;

pub type ShortHash = u8;

pub fn full_hash(line: &str) -> u32 {
    // Strip trailing whitespace (including CR) before hashing.
    //
    // This makes anchors stable across formatter runs that adjust trailing
    // whitespace (Prettier, Black, gofmt, etc.) and across CRLF/LF newline
    // changes. Matches the behavior of oh-my-pi, hashfile-mcp,
    // mcp-hashline-edit-server, and pi-hashline-edit.
    full_hash_bytes(line.trim_end().as_bytes())
}

pub fn full_hash_bytes(bytes: &[u8]) -> u32 {
    xxh32(bytes, 0)
}

/// 64-bit content hash using xxh3 — 2-4x faster than xxh32 on modern
/// CPUs with SIMD (SSE2/AVX2). Used by the hash sidecar for collision
/// resistance at no extra cost. Not used for anchor generation (short
/// hashes remain xxh32 for backward compatibility).
pub fn full_hash64(line: &str) -> u64 {
    full_hash_bytes64(line.trim_end().as_bytes())
}

/// Raw xxh3 64-bit hash for byte slices. Approximately 2-4x faster
/// than xxh32 on modern x86_64 and arm64 hardware.
pub fn full_hash_bytes64(bytes: &[u8]) -> u64 {
    xxh3::xxh3_64(bytes)
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
    let mut buf = [0u8; 2];
    write_short_hash_bytes(&mut buf, short);
    // SAFETY: write_short_hash_bytes writes only ASCII hex digits, which are
    // always valid UTF-8. This avoids the cost of `format!`'s general
    // formatting machinery on a path that's called once per hot grep/read
    // match (100k+ times on a 100k-line file).
    unsafe { String::from_utf8_unchecked(buf.to_vec()) }
}

/// Write the 2-character lowercase hex representation of `short` into `buf`.
///
/// Exposed so hot output paths can render anchors straight into a stdout
/// byte buffer without going through `format!` or allocating a temporary
/// `String`.
#[inline]
pub fn write_short_hash_bytes(buf: &mut [u8; 2], short: ShortHash) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[0] = HEX[(short >> 4) as usize];
    buf[1] = HEX[(short & 0x0f) as usize];
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
    fn test_trailing_space_does_not_affect_hash() {
        // After Phase 1: trailing whitespace is stripped before hashing.
        // This keeps anchors stable across formatter runs (Prettier, Black, gofmt, etc.)
        assert_eq!(short_hash("return decoded "), short_hash("return decoded"));
        assert_eq!(
            short_hash("return decoded   "),
            short_hash("return decoded")
        );
        assert_eq!(short_hash("return decoded\t"), short_hash("return decoded"));
    }

    #[test]
    fn test_leading_space_still_affects_hash() {
        // Leading whitespace (indentation) is meaningful — formatter changes
        // to indentation should still invalidate anchors.
        assert_ne!(short_hash("  return decoded"), short_hash("return decoded"));
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
        // After Phase 1: trailing CR (and any whitespace) is stripped before hashing.
        // This makes hashes stable across CRLF/LF newline conversions.
        assert_eq!(short_hash("line"), short_hash("line\r"));
        assert_eq!(short_hash("line"), short_hash("line\r\n"));
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
