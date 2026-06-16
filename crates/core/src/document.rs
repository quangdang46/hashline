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
    pub fn lines_with_hashes(&self) -> Vec<LineEntry> {
        let lines = self.lines();
        lines
            .iter()
            .map(|line| LineEntry {
                content: line.to_string(),
                short_hash: hash::short_hash_value(line),
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

#[derive(Clone, Debug)]
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

    fn write_temp_file(content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, content).unwrap();
        (dir, path)
    }
}
