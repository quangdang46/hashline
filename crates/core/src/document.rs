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

/// Number of lines each worker handles in the parallel path. Also used as
/// the granularity for sizing the worker count: a 4096-line file gets ~2
/// workers, and the count never exceeds [`MAX_HASH_WORKERS`] no matter how
/// large the file is.
const PARALLEL_HASH_CHUNK_LINES: usize = 2048;

/// Hard cap on hashing worker threads. The old implementation spawned one
/// thread per 2048-line chunk — ~244 threads on a 500k-line file — which is
/// pure overhead: xxh32 hashing is memory-bandwidth-bound well below that,
/// and each spawn costs a kernel transition plus stack setup.
const MAX_HASH_WORKERS: usize = 16;

/// Lightweight in-memory file content, normalized to LF and BOM-stripped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContent {
    pub path: PathBuf,
    /// Raw text as read from disk (before any normalization).
    pub raw: String,
    /// LF-normalized, BOM-stripped text.
    pub normalized: String,
    /// The UTF-8 BOM marker stripped from `raw` — `"\u{FEFF}"` when present,
    /// empty otherwise. Write-back paths re-attach it so a patch/write does
    /// not silently strip the file's encoding marker.
    pub bom: String,
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
                bom: String::new(),
                newline: NewlineStyle::Lf,
                trailing_newline: false,
                hash: hash::compute_file_hash(""),
            });
        }

        // Binary-file check on the first 8 KiB.
        if memchr(0, &bytes[..bytes.len().min(8_000)]).is_some() {
            return Err(HashlineError::BinaryFile { path: path_string });
        }

        let text = std::str::from_utf8(&bytes).map_err(|_| HashlineError::InvalidUtf8 {
            path: path_string.clone(),
        })?;

        // Strip the BOM on a borrowed slice — no copy. `raw` keeps the
        // BOM'd original (write-back paths re-attach `bom` themselves).
        let (bom, body) = if let Some(rest) = text.strip_prefix('\u{FEFF}') {
            ("\u{FEFF}", rest)
        } else {
            ("", text)
        };

        // Fast path: a file with no `'\r'` byte is already LF-normalized —
        // `normalized` is a plain memcpy and the dominant ending is LF by
        // definition (no CR/CRLF terminators can exist). This skips both
        // the per-char/byte normalize pass and the full EOL histogram scan
        // for the overwhelming majority of files. The `memchr` probe is
        // SIMD-speed, so even CRLF files only pay one extra scan.
        let (newline, normalized) = if memchr(b'\r', body.as_bytes()).is_some() {
            (
                NewlineStyle::from_normalize(crate::normalize::detect_line_ending(body)),
                crate::normalize::normalize_to_lf(body),
            )
        } else {
            (NewlineStyle::Lf, body.to_owned())
        };
        // `trailing_newline` must be computed on the *normalized* text, not
        // the raw bytes: a pure-CR file ("a\rb\r") has no '\n' in raw form,
        // so checking `raw.ends_with('\n')` reported false while
        // `normalized` ended with '\n' — producing a phantom trailing empty
        // line in `read`/`find-block` output that `patch` then rejected.
        let trailing_newline = normalized.ends_with('\n');
        let hash_val = hash::compute_file_hash(&normalized);
        // One owned copy of the on-disk text (BOM included); `normalized`
        // above is the second. Previously this path made three full copies
        // (raw + strip_bom + normalize_to_lf).
        let raw = text.to_owned();

        Ok(FileContent {
            path: path.to_path_buf(),
            raw,
            normalized,
            bom: bom.to_owned(),
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

    /// Line entries with content but a **zeroed** `short_hash`.
    ///
    /// For apply paths whose edits carry no hash-qualified anchors: the
    /// applier only reads `short_hash` when an `expected_hash` is present,
    /// so the per-line xxh32 + symbol scan is pure waste there. This still
    /// clones every line (block resolution and `CUT` capture read content),
    /// it only skips the hashing. Never use for `read`, anchor resolution,
    /// or anywhere a real hash is consumed.
    pub fn lines_without_hashes(&self) -> Vec<LineEntry> {
        self.lines()
            .iter()
            .map(|line| LineEntry {
                content: line.to_string(),
                short_hash: 0,
            })
            .collect()
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
        // Partition ALL lines into `workers` contiguous ranges — one spawn
        // per worker, each hashing its whole range serially. The line
        // number (and thus the position seed for symbol-only lines) derives
        // from the global index, never from a worker id, so the output is
        // bit-for-bit identical to the serial path.
        let n = lines.len();
        let workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1)
            .min(MAX_HASH_WORKERS)
            .min(n.div_ceil(PARALLEL_HASH_CHUNK_LINES))
            .max(1);
        if workers <= 1 {
            return self.lines_with_hashes_serial(lines);
        }

        let base = n / workers;
        let extra = n % workers; // first `extra` workers get one more line

        let mut hashes = vec![0u8; n];
        std::thread::scope(|s| {
            let mut line_rest: &[&str] = lines;
            let mut hash_rest: &mut [u8] = &mut hashes;
            let mut line_offset = 0usize; // 0-based index of range start
            for w in 0..workers {
                let count = base + usize::from(w < extra);
                let (line_chunk, lrest) = line_rest.split_at(count);
                line_rest = lrest;
                let (hash_chunk, hrest) = hash_rest.split_at_mut(count);
                hash_rest = hrest;
                // 1-based line number of the first line in this range.
                let first_line_no = line_offset + 1;
                line_offset += count;
                s.spawn(move || {
                    for (j, (&line, slot)) in
                        line_chunk.iter().zip(hash_chunk.iter_mut()).enumerate()
                    {
                        *slot = hash::short_hash_value_indexed(line, first_line_no + j);
                    }
                });
            }
        });

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
    /// Bare-CR (classic Mac) line endings.
    Cr,
}

impl NewlineStyle {
    pub fn separator(self) -> &'static str {
        match self {
            NewlineStyle::Lf => "\n",
            NewlineStyle::Crlf => "\r\n",
            NewlineStyle::Cr => "\r",
        }
    }

    fn from_normalize(le: crate::normalize::LineEnding) -> Self {
        match le {
            crate::normalize::LineEnding::Lf => NewlineStyle::Lf,
            crate::normalize::LineEnding::Crlf => NewlineStyle::Crlf,
            crate::normalize::LineEnding::Cr => NewlineStyle::Cr,
        }
    }
}

/// Probe an existing file for its BOM marker and dominant line-ending
/// style. Overwrite paths (`write --force`, `Editor::write`, MCP `write`)
/// call this so replacing a file does not silently change its encoding
/// conventions. Returns `("", LineEnding::Lf)` when the file is missing,
/// unreadable, or not valid UTF-8.
pub fn probe_existing_style(path: &Path) -> (String, crate::normalize::LineEnding) {
    let fallback = || (String::new(), crate::normalize::LineEnding::Lf);
    let Ok(bytes) = fs::read(path) else {
        return fallback();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return fallback();
    };
    crate::normalize::detect_file_style(text)
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
            bom: String::new(),
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

    // -- bd-eol-bom-u7h: BOM / EOL preservation ----------------------------

    #[test]
    fn test_load_bom_is_captured_and_normalized_strips_it() {
        let (_dir, path) = write_temp_file("\u{FEFF}alpha\nbeta\n");
        let fc = FileContent::load(&path).unwrap();

        assert_eq!(fc.bom, "\u{FEFF}");
        assert_eq!(fc.normalized, "alpha\nbeta\n");
        // The file hash is computed over the normalized text, so a BOM
        // file and its BOM-less twin report the same hash.
        assert_eq!(fc.hash, hash::compute_file_hash("alpha\nbeta\n"));
    }

    #[test]
    fn test_load_no_bom_gives_empty_marker() {
        let (_dir, path) = write_temp_file("alpha\n");
        let fc = FileContent::load(&path).unwrap();
        assert_eq!(fc.bom, "");
    }

    #[test]
    fn test_load_pure_cr_file() {
        // Classic Mac OS line endings: every '\r' is a terminator.
        let (_dir, path) = write_temp_file("alpha\rbeta\r");
        let fc = FileContent::load(&path).unwrap();

        assert_eq!(fc.newline, NewlineStyle::Cr);
        assert_eq!(fc.normalized, "alpha\nbeta\n");
        // The trailing '\r' IS a terminator — this is what the old
        // raw-bytes check got wrong (it looked for '\n' only).
        assert!(fc.trailing_newline);
        assert_eq!(fc.lines(), vec!["alpha", "beta", ""]);
    }

    #[test]
    fn test_load_pure_cr_no_trailing_terminator() {
        let (_dir, path) = write_temp_file("alpha\rbeta");
        let fc = FileContent::load(&path).unwrap();

        assert_eq!(fc.newline, NewlineStyle::Cr);
        assert!(!fc.trailing_newline);
        assert_eq!(fc.lines(), vec!["alpha", "beta"]);
    }

    #[test]
    fn test_load_mixed_eol_detects_dominant_not_first() {
        // Two LF + one CRLF: dominant is LF even though CRLF appears first.
        // The old first-ending-wins detection returned Crlf here.
        let (_dir, path) = write_temp_file("a\r\nb\nc\n");
        let fc = FileContent::load(&path).unwrap();
        assert_eq!(fc.newline, NewlineStyle::Lf);

        // Two CRLF + one LF: dominant is CRLF.
        let (_dir2, path2) = write_temp_file("a\r\nb\r\nc\n");
        let fc2 = FileContent::load(&path2).unwrap();
        assert_eq!(fc2.newline, NewlineStyle::Crlf);
    }

    #[test]
    fn test_probe_existing_style() {
        let (_dir, path) = write_temp_file("\u{FEFF}a\r\nb\r\n");
        let (bom, ending) = probe_existing_style(&path);
        assert_eq!(bom, "\u{FEFF}");
        assert_eq!(ending, crate::normalize::LineEnding::Crlf);

        let (_dir2, path2) = write_temp_file("a\rb\r");
        let (bom2, ending2) = probe_existing_style(&path2);
        assert_eq!(bom2, "");
        assert_eq!(ending2, crate::normalize::LineEnding::Cr);

        // Missing file → LF/no-BOM default.
        let (bom3, ending3) = probe_existing_style(Path::new("does-not-exist-xyz"));
        assert_eq!(bom3, "");
        assert_eq!(ending3, crate::normalize::LineEnding::Lf);
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
            bom: String::new(),
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
