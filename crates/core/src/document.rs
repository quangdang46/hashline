//! In-memory file content, normalized to LF and BOM-stripped.
//!
//! The primary representation used by hashline commands. Unlike the legacy
//! `Document` type (removed), this struct does **not** pre-hash every line —
//! it keeps the raw text and a single 4-hex file-content hash.

use std::fs;
use std::path::{Path, PathBuf};

use memchr::memchr;
use memmap2::Mmap;

use crate::error::HashlineError;
use crate::hash;

// ---------------------------------------------------------------------------
// FileContent — single in-memory file representation
// ---------------------------------------------------------------------------

/// Minimum number of lines before `lines_with_hashes` fans out to worker
/// threads. Below this, thread-spawn overhead dominates and the single-threaded
/// path is faster — small files (and most test fixtures) stay on it.
const PARALLEL_HASH_MIN_LINES: usize = 4096;

/// Number of lines each worker handles in the parallel path. This also caps
/// the thread count: a 4096-line file spawns 2 threads, a 100k-line file ~49.
const PARALLEL_HASH_CHUNK_LINES: usize = 2048;

/// Lightweight in-memory file content, normalized to LF and BOM-stripped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContent {
    pub path: PathBuf,
    /// Raw text as read from disk (before any normalization).
    pub raw: String,
    /// LF-normalized, BOM-stripped text.
    pub normalized: String,
    /// Detected line ending style from the raw content.
    pub newline: NewlineStyle,
    /// Whether the raw content ends with a newline.
    pub trailing_newline: bool,
    /// 4-hex content hash computed over the **normalized** text.
    pub hash: String,
}

impl FileContent {
    /// Read `path`, strip BOM, detect line endings, normalize to LF, compute
    /// the 4-hex content hash, and return a [`FileContent`].
    pub fn load(path: &Path) -> Result<Self, HashlineError> {
        let path_string = path.display().to_string();
        let file = fs::File::open(path)?;
        let bytes = unsafe { Mmap::map(&file) }?;

        if bytes.is_empty() {
            return Ok(FileContent {
                path: path.to_path_buf(),
                raw: String::new(),
                normalized: String::new(),
                newline: NewlineStyle::Lf,
                trailing_newline: false,
                hash: hash::compute_file_hash(""),
            });
        }

        // Binary-file check on the first 8 KiB.
        if memchr(0, &bytes[..bytes.len().min(8_000)]).is_some() {
            return Err(HashlineError::BinaryFile { path: path_string });
        }

        let raw = std::str::from_utf8(&bytes)
            .map_err(|_| HashlineError::InvalidUtf8 {
                path: path_string.clone(),
            })?
            .to_owned();

        let bom = crate::normalize::strip_bom(&raw);
        let trailing_newline = bom.text.ends_with('\n');
        let newline = NewlineStyle::from_normalize(crate::normalize::detect_line_ending(&bom.text));
        let normalized = crate::normalize::normalize_to_lf(&bom.text);
        let hash_val = hash::compute_file_hash(&normalized);

        Ok(FileContent {
            path: path.to_path_buf(),
            raw,
            normalized,
            newline,
            trailing_newline,
            hash: hash_val,
        })
    }

    /// Return the lines of the **normalized** text (split on `'\n'`).
    ///
    /// This is O(N) and allocates a `Vec`; prefer iterating over the string
    /// directly when you only need a few lines.
    pub fn lines(&self) -> Vec<&str> {
        if self.normalized.is_empty() {
            return Vec::new();
        }
        self.normalized.split('\n').collect()
    }

    /// Return the lines with per-line short hashes computed.
    ///
    /// This allocates a `Vec<LineEntry>` with one entry per line. The
    /// trailing empty line from `split('\n')` is included when the file
    /// ends with a newline.
    ///
    /// On large files the per-line hash computation is fanned out across
    /// worker threads (see [`PARALLEL_HASH_MIN_LINES`]). Each line's short
    /// hash depends only on its own bytes and its 1-based line number, so
    /// workers own disjoint line ranges and the result is bit-for-bit
    /// identical to the serial path; small files stay on the serial path to
    /// avoid thread-spawn overhead.
    pub fn lines_with_hashes(&self) -> Vec<LineEntry> {
        let lines = self.lines();
        if lines.len() < PARALLEL_HASH_MIN_LINES {
            return self.lines_with_hashes_serial(&lines);
        }
        self.lines_with_hashes_parallel(&lines)
    }

    /// Serial reference implementation — used directly for small files and as
    /// the exact-output oracle for the parallel path in tests.
    fn lines_with_hashes_serial(&self, lines: &[&str]) -> Vec<LineEntry> {
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| LineEntry {
                content: line.to_string(),
                // Position-seeded for symbol-only lines so identical `}` etc.
                // disambiguate; content lines keep the plain hash. 1-based line
                // number matches the read/patch anchor convention.
                short_hash: hash::short_hash_value_indexed(line, i + 1),
            })
            .collect()
    }

    /// Parallel variant: hash each line's short hash on worker threads, then
    /// materialize the `LineEntry`s. The returned `Vec` is identical to
    /// [`FileContent::lines_with_hashes_serial`].
    ///
    /// Only the hash computation runs on worker threads — a worker writes one
    /// `u8` per line into a pre-allocated `Vec`, at disjoint indices, so the
    /// whole fan-out is safe Rust. The `String` copies are built serially
    /// afterwards (hashing is the dominant cost; see
    /// `bench-results/accuracy-branch-2026-08-10.md`).
    fn lines_with_hashes_parallel(&self, lines: &[&str]) -> Vec<LineEntry> {
        // Split off the full chunks (hashed on worker threads) from a remainder
        // shorter than one chunk (hashed inline after the workers join, so we
        // don't spawn a thread for a handful of lines).
        let full_len = lines.len() - lines.len() % PARALLEL_HASH_CHUNK_LINES;

        let mut hashes = vec![0u8; lines.len()];
        let (full_hashes, tail_hashes) = hashes.split_at_mut(full_len);

        std::thread::scope(|s| {
            // Each worker owns a disjoint chunk of `lines` and a disjoint
            // `&mut [u8]` slice of `full_hashes`, so no two threads write the
            // same index. `split_at_mut` guarantees the per-iteration `out`
            // slice is disjoint from everything the loop keeps in `remaining`.
            // The line number (and thus the position seed for symbol-only
            // lines) derives from the global index
            // `chunk_index * CHUNK + j`, never from a thread id.
            let mut remaining = &mut full_hashes[..];
            for (chunk_index, chunk) in lines[..full_len]
                .chunks(PARALLEL_HASH_CHUNK_LINES)
                .enumerate()
            {
                let (out, rest) = remaining.split_at_mut(PARALLEL_HASH_CHUNK_LINES);
                remaining = rest;
                s.spawn(move || {
                    for (j, (&line, slot)) in chunk.iter().zip(out.iter_mut()).enumerate() {
                        *slot = hash::short_hash_value_indexed(
                            line,
                            chunk_index * PARALLEL_HASH_CHUNK_LINES + j + 1,
                        );
                    }
                });
            }
        });

        // Remainder lines — hashed here, after all workers have joined.
        for (j, &line) in lines[full_len..].iter().enumerate() {
            tail_hashes[j] = hash::short_hash_value_indexed(line, full_len + j + 1);
        }

        lines
            .iter()
            .zip(hashes)
            .map(|(&line, short_hash)| LineEntry {
                content: line.to_string(),
                short_hash,
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        if self.normalized.is_empty() {
            return 0;
        }
        // Count newlines — that gives the number of lines (which equals
        // the number of splits).
        self.normalized.bytes().filter(|&b| b == b'\n').count()
            + usize::from(!self.normalized.ends_with('\n'))
    }

    pub fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }
}

// ---------------------------------------------------------------------------
// LineEntry — per-line content + short hash, built on demand
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineEntry {
    pub content: String,
    pub short_hash: u8,
}

// ---------------------------------------------------------------------------
// NewlineStyle
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewlineStyle {
    Lf,
    Crlf,
}

impl NewlineStyle {
    pub fn separator(self) -> &'static str {
        match self {
            NewlineStyle::Lf => "\n",
            NewlineStyle::Crlf => "\r\n",
        }
    }

    fn from_normalize(le: crate::normalize::LineEnding) -> Self {
        match le {
            crate::normalize::LineEnding::Lf => NewlineStyle::Lf,
            crate::normalize::LineEnding::Crlf => NewlineStyle::Crlf,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_lf_simple() {
        let (_dir, path) = write_temp_file("alpha\nbeta\n");
        let fc = FileContent::load(&path).unwrap();

        assert_eq!(fc.newline, NewlineStyle::Lf);
        assert!(fc.trailing_newline);
        assert_eq!(fc.lines(), vec!["alpha", "beta", ""]);
    }

    #[test]
    fn test_load_single_line_no_trailing_newline() {
        let (_dir, path) = write_temp_file("alpha");
        let fc = FileContent::load(&path).unwrap();

        assert!(!fc.trailing_newline);
        assert_eq!(fc.lines(), vec!["alpha"]);
    }

    #[test]
    fn test_load_empty_file() {
        let (_dir, path) = write_temp_file("");
        let fc = FileContent::load(&path).unwrap();

        assert!(!fc.trailing_newline);
        assert!(fc.lines().is_empty());
        assert!(fc.is_empty());
    }

    #[test]
    fn test_lines_with_hashes_includes_all_lines() {
        let fc = FileContent {
            path: PathBuf::from("demo.txt"),
            raw: "a\nb\n".into(),
            normalized: "a\nb\n".into(),
            newline: NewlineStyle::Lf,
            trailing_newline: true,
            hash: "abcd".into(),
        };
        let entries = fc.lines_with_hashes();
        assert_eq!(entries.len(), 3); // "a", "b", ""
        assert_eq!(entries[0].content, "a");
        assert_eq!(entries[1].content, "b");
        assert_eq!(entries[2].content, "");
    }

    #[test]
    fn test_len_matches_line_count() {
        let (_dir, path) = write_temp_file("a\nb\nc\n");
        let fc = FileContent::load(&path).unwrap();
        assert_eq!(fc.len(), 3);

        let (_dir2, path2) = write_temp_file("single");
        let fc2 = FileContent::load(&path2).unwrap();
        assert_eq!(fc2.len(), 1);
    }

    // -- Phase 8: parallel line hashing ------------------------------------

    /// Serial reference implementation, kept inline so the test does not rely
    /// on the code under test. `i + 1` is the 1-based line number feeding the
    /// position seed — identical to the production contract.
    fn serial_lines_with_hashes(lines: &[&str]) -> Vec<LineEntry> {
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| LineEntry {
                content: line.to_string(),
                short_hash: hash::short_hash_value_indexed(line, i + 1),
            })
            .collect()
    }

    #[test]
    fn test_parallel_lines_with_hashes_matches_serial() {
        // A large fixture (well past PARALLEL_HASH_MIN_LINES and past the
        // 2048-line chunk boundary, so the remainder path runs too) with a
        // non-chunk-multiple line count. Every line — content, symbol-only,
        // blank — must hash bit-for-bit identically to the serial path.
        let n = PARALLEL_HASH_MIN_LINES + PARALLEL_HASH_CHUNK_LINES + 100;
        let text = symbol_mixed_fixture(n);
        let fc = make_fc(&text);
        let entries = fc.lines_with_hashes();
        let serial = serial_lines_with_hashes(&fc.lines());

        assert_eq!(entries.len(), n + 1); // trailing newline adds the "" line
        assert_eq!(entries, serial);
    }

    #[test]
    fn test_parallel_restores_serial_for_very_large_file() {
        // Round-trip sanity: after hashing a huge parallel file, re-hash it and
        // confirm determinism (the parallel output must be a pure function of
        // the input — two runs agree, and both match serial on the same text).
        let text = symbol_mixed_fixture(PARALLEL_HASH_MIN_LINES * 3);
        let fc = make_fc(&text);
        let first = fc.lines_with_hashes();
        let second = fc.lines_with_hashes();
        assert_eq!(first, second);

        let serial = serial_lines_with_hashes(&fc.lines());
        assert_eq!(first, serial);
    }

    #[test]
    fn test_small_file_below_thread_threshold_still_works() {
        // Files under PARALLEL_HASH_MIN_LINES take the serial path; the output
        // must be unchanged and include the trailing empty line.
        let fc = make_fc("a\n}\n\n");
        let entries = fc.lines_with_hashes();
        assert_eq!(entries.len(), 4); // "a", "}", "", ""
        assert_eq!(entries, serial_lines_with_hashes(&fc.lines()));
    }

    #[test]
    fn test_parallel_symbol_lines_yield_256_distinct_hashes() {
        // The Phase 2 property on a 5000-line symbol fixture must survive the
        // parallel path: identical `}`/`)`/blank lines spread across the full
        // 256-value short-hash space (position-seeded), never collapsing to a
        // handful of values. This is the `accuracy_bench` symbol metric.
        use std::collections::HashSet;

        let text = symbol_mixed_fixture(5000);
        let fc = make_fc(&text);
        let entries = fc.lines_with_hashes();

        let mut distinct: HashSet<u8> = HashSet::new();
        for e in &entries {
            distinct.insert(e.short_hash);
        }
        assert_eq!(
            distinct.len(),
            256,
            "5000 position-seeded symbol lines should span all 256 short hashes"
        );

        // And it must agree with the serial reference on this fixture.
        assert_eq!(entries, serial_lines_with_hashes(&fc.lines()));
    }

    fn write_temp_file(content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn make_fc(content: &str) -> FileContent {
        FileContent {
            path: PathBuf::from("demo.txt"),
            raw: content.to_string(),
            normalized: content.to_string(),
            newline: NewlineStyle::Lf,
            trailing_newline: content.ends_with('\n'),
            hash: "0000".into(),
        }
    }

    /// Deterministic fixture with a mix of content, symbol-only, and blank
    /// lines (mirrors the `accuracy_bench` symbol fixture) so the position
    /// seed exercises both branches of `short_hash_value_indexed`.
    fn symbol_mixed_fixture(line_count: usize) -> String {
        let mut lines = Vec::with_capacity(line_count);
        for i in 0..line_count {
            lines.push(match i % 5 {
                0 => "}".to_string(),
                1 => ")".to_string(),
                2 => String::new(),
                3 => format!("fn generated_line_{i:05}() {{ }}"),
                _ => format!("let value_{i:05} = {};", i * 7),
            });
        }
        lines.join("\n") + "\n"
    }
}
