#![allow(dead_code)]

use std::io::{BufRead, Write};
use std::path::Path;

use tempfile::NamedTempFile;

use crate::document::{Document, LineRecord, NewlineStyle};
use crate::error::HashlineError;
use crate::hash::{self, ShortHash};

pub fn validate_single_line_content(content: &str) -> Result<(), HashlineError> {
    if content.contains(['\n', '\r']) {
        Err(HashlineError::MultiLineContentUnsupported)
    } else {
        Ok(())
    }
}

pub fn split_content_lines(content: &str) -> Vec<Box<str>> {
    if content.is_empty() {
        return vec![Box::from("")];
    }

    let lines = content.lines().map(Box::from).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![Box::from("")]
    } else {
        lines
    }
}

pub fn replace_line(doc: &mut Document, index: usize, content: &str) -> Result<(), HashlineError> {
    validate_single_line_content(content)?;
    ensure_index(doc, index)?;

    let old_len = doc.lines[index].content.len();
    if doc.lines[index].content.as_ref() != content {
        doc.lines[index].content = Box::from(content);
        refresh_line_metadata(&mut doc.lines[index]);
    }
    doc.content_len = doc.content_len + doc.lines[index].content.len() - old_len;
    Ok(())
}

pub fn replace_range_with_line(
    doc: &mut Document,
    start: usize,
    end: usize,
    content: &str,
) -> Result<(), HashlineError> {
    validate_single_line_content(content)?;
    replace_range(doc, start, end, content)
}

pub fn replace_range(
    doc: &mut Document,
    start: usize,
    end: usize,
    content: &str,
) -> Result<(), HashlineError> {
    ensure_range(doc, start, end)?;
    let replacement = split_content_lines(content);

    let removed_len: usize = doc.lines[start..=end]
        .iter()
        .map(|line| line.content.len())
        .sum();
    let inserted_len: usize = replacement.iter().map(|line| line.len()).sum();
    doc.lines.splice(
        start..=end,
        replacement.iter().map(|line| new_line_record(line)),
    );
    doc.content_len = doc.content_len + inserted_len - removed_len;
    Ok(())
}

pub fn insert_line(doc: &mut Document, index: usize, content: &str) -> Result<(), HashlineError> {
    ensure_insert_index(doc, index)?;

    let lines = split_content_lines(content);
    let total_len: usize = lines.iter().map(|l| l.len()).sum();

    for (i, line) in lines.into_iter().enumerate() {
        let insert_at = index + i;
        doc.lines.insert(insert_at, new_line_record(&line));
        refresh_line_metadata(&mut doc.lines[insert_at]);
    }
    doc.content_len += total_len;
    Ok(())
}

pub fn delete_line(doc: &mut Document, index: usize) -> Result<(), HashlineError> {
    ensure_index(doc, index)?;

    let removed_len = doc.lines[index].content.len();
    doc.lines.remove(index);
    doc.content_len -= removed_len;
    Ok(())
}

pub fn delete_range(doc: &mut Document, start: usize, end: usize) -> Result<(), HashlineError> {
    ensure_range(doc, start, end)?;

    let removed_len: usize = doc.lines[start..=end]
        .iter()
        .map(|line| line.content.len())
        .sum();
    doc.lines.drain(start..=end);
    doc.content_len -= removed_len;
    Ok(())
}

pub fn swap_lines(doc: &mut Document, left: usize, right: usize) -> Result<(), HashlineError> {
    ensure_index(doc, left)?;
    ensure_index(doc, right)?;

    if left == right {
        return Err(HashlineError::PatchFailed {
            op_index: 0,
            reason: "source and target must resolve to different lines".to_owned(),
        });
    }

    doc.lines.swap(left, right);
    Ok(())
}

pub fn move_line(
    doc: &mut Document,
    source: usize,
    target: usize,
    place_before: bool,
) -> Result<usize, HashlineError> {
    ensure_index(doc, source)?;
    ensure_index(doc, target)?;

    if source == target {
        return Err(HashlineError::PatchFailed {
            op_index: 0,
            reason: "source and target must resolve to different lines".to_owned(),
        });
    }

    let line = doc.lines.remove(source);
    let adjusted_target = if source < target { target - 1 } else { target };
    let insert_at = if place_before {
        adjusted_target
    } else {
        adjusted_target + 1
    };

    doc.lines.insert(insert_at, line);
    Ok(insert_at)
}

fn refresh_line_metadata(line: &mut LineRecord) {
    line.short_hash = hash::short_from_full(hash::full_hash(&line.content));
}

fn new_line_record(content: &str) -> LineRecord {
    let full_hash = hash::full_hash(content);
    LineRecord {
        content: Box::from(content),
        short_hash: hash::short_from_full(full_hash),
    }
}

fn ensure_index(doc: &Document, index: usize) -> Result<(), HashlineError> {
    if index < doc.lines.len() {
        Ok(())
    } else {
        Err(HashlineError::MutationIndexOutOfBounds {
            index,
            len: doc.lines.len(),
        })
    }
}

fn ensure_insert_index(doc: &Document, index: usize) -> Result<(), HashlineError> {
    if index <= doc.lines.len() {
        Ok(())
    } else {
        Err(HashlineError::MutationIndexOutOfBounds {
            index,
            len: doc.lines.len(),
        })
    }
}

fn ensure_range(doc: &Document, start: usize, end: usize) -> Result<(), HashlineError> {
    if start <= end && end < doc.lines.len() {
        Ok(())
    } else {
        Err(HashlineError::InvalidMutationRange {
            start,
            end,
            len: doc.lines.len(),
        })
    }
}

/// Stream `path` line-by-line, replacing the line at `target_line` (0-indexed)
/// with `new_content`, writing the result to a temp file and then atomically
/// replacing the original.
///
/// Before replacing, this function reads the target line, computes its
/// short hash, and compares it against `expected_hash`. If the hashes
/// differ, the file has been modified since the anchor was obtained, and
/// a [`HashlineError::StaleAnchor`] is returned.
///
/// # Constraints
///
/// - `new_content` must be a single line (no `\n` or `\r`).
/// - `expected_hash` must be the 1-byte short hash to verify the target line.
///
/// # Memory
///
/// This function uses a BufReader and BufWriter, streaming one line at a
/// time. The full file is never held in memory. The only notable allocation
/// is `new_content` and the per-line read buffer (which is reused across
/// iterations).
pub fn stream_replace_line(
    path: &Path,
    target_line: usize,
    new_content: &str,
    expected_hash: ShortHash,
    newline: NewlineStyle,
    trailing_newline: bool,
) -> Result<(), HashlineError> {
    validate_single_line_content(new_content)?;

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let separator = newline.separator().as_bytes();

    // Write to a temp file in the same directory so that we can atomically
    // rename it over the original.
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    let existing_permissions = std::fs::metadata(path).ok().map(|meta| meta.permissions());
    if let Some(permissions) = existing_permissions {
        temp.as_file().set_permissions(permissions)?;
    }

    let mut lines_seen = 0usize;
    let mut anchor_verified = false;
    // Reusable read buffer across all lines.
    let mut buf = Vec::<u8>::new();

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }

        let line_content = if buf.ends_with(b"\r\n") {
            // CRLF: strip \r\n before hashing
            std::str::from_utf8(&buf[..buf.len() - 2])
        } else if buf.ends_with(b"\n") {
            // LF: strip \n
            std::str::from_utf8(&buf[..buf.len() - 1])
        } else {
            // No newline (trailing line, no trailing newline in file)
            std::str::from_utf8(&buf)
        }
        .map_err(|_| HashlineError::InvalidUtf8 {
            path: path.display().to_string(),
        })?;

        if lines_seen == target_line {
            // Verify the hash before replacing.
            let actual_hash = hash::short_hash_value(line_content);
            if actual_hash != expected_hash {
                return Err(HashlineError::StaleAnchor {
                    anchor: format!(
                        "{}:{}",
                        target_line + 1,
                        hash::format_short_hash(expected_hash)
                    )
                    .into(),
                    line: target_line + 1,
                    expected: hash::format_short_hash(expected_hash).into(),
                    actual: hash::format_short_hash(actual_hash).into(),
                    path: path.display().to_string().into(),
                    relocated_suffix: "".into(),
                });
            }
            anchor_verified = true;

            // Write the replacement line (without the separator since
            // the separator between lines is handled by the inter-line logic).
            if lines_seen > 0 {
                temp.write_all(separator)?;
            }
            temp.write_all(new_content.as_bytes())?;
        } else {
            // Echo back the original line.
            if lines_seen > 0 {
                temp.write_all(separator)?;
            }
            temp.write_all(line_content.as_bytes())?;
        }
        lines_seen += 1;
    }

    if !anchor_verified {
        return Err(HashlineError::MutationIndexOutOfBounds {
            index: target_line,
            len: lines_seen,
        });
    }

    // Preserve the trailing newline.
    if trailing_newline {
        temp.write_all(separator)?;
    }

    temp.flush()?;
    temp.as_file().sync_all()?;

    // Atomically replace the original file.
    temp.persist(path)
        .map_err(|e| HashlineError::Io(e.error))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        delete_line, delete_range, insert_line, move_line, replace_line, replace_range,
        replace_range_with_line, split_content_lines, swap_lines, validate_single_line_content,
    };
    use crate::document::{Document, NewlineStyle};
    use crate::error::HashlineError;
    use std::path::Path;

    #[test]
    fn replace_line_recomputes_hashes_and_preserves_document_flags() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let original_newline = doc.newline;
        let original_trailing_newline = doc.trailing_newline;

        replace_line(&mut doc, 1, "gamma").unwrap();

        assert_eq!(doc.lines[1].content.as_ref(), "gamma");
        assert_eq!(doc.newline, original_newline);
        assert_eq!(doc.trailing_newline, original_trailing_newline);
        assert_eq!(doc.render(), b"alpha\ngamma\n");
    }

    #[test]
    fn replace_range_collapses_to_single_line() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();

        replace_range_with_line(&mut doc, 1, 2, "merged").unwrap();

        assert_eq!(doc.lines.len(), 3);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "merged");
        assert_eq!(doc.lines[2].content.as_ref(), "delta");
        assert_eq!(doc.render(), b"alpha\nmerged\ndelta\n");
    }

    #[test]
    fn replace_range_expands_to_multiple_lines() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();

        replace_range(&mut doc, 1, 2, "left\nmiddle\nright").unwrap();

        assert_eq!(doc.lines.len(), 5);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "left");
        assert_eq!(doc.lines[2].content.as_ref(), "middle");
        assert_eq!(doc.lines[3].content.as_ref(), "right");
        assert_eq!(doc.lines[4].content.as_ref(), "delta");
        assert_eq!(doc.render(), b"alpha\nleft\nmiddle\nright\ndelta\n");
    }

    #[test]
    fn insert_line_at_index_renumbers_following_lines() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\ngamma\n").unwrap();
        let original_hash = doc.lines[1].short_hash;

        insert_line(&mut doc, 1, "beta").unwrap();

        assert_eq!(doc.lines.len(), 3);
        assert_eq!(doc.lines[1].content.as_ref(), "beta");
        assert_eq!(doc.lines[2].content.as_ref(), "gamma");
        assert_eq!(doc.lines[2].short_hash, original_hash);
        assert_eq!(doc.render(), b"alpha\nbeta\ngamma\n");
    }

    #[test]
    fn insert_line_allows_appending_to_end() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        insert_line(&mut doc, 1, "beta").unwrap();

        assert_eq!(doc.render(), b"alpha\nbeta\n");
    }

    #[test]
    fn insert_line_allows_multiline_content() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\ndelta\n").unwrap();

        insert_line(&mut doc, 1, "beta\ngamma").unwrap();

        assert_eq!(doc.lines.len(), 4);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "beta");
        assert_eq!(doc.lines[2].content.as_ref(), "gamma");
        assert_eq!(doc.lines[3].content.as_ref(), "delta");
        assert_eq!(doc.render(), b"alpha\nbeta\ngamma\ndelta\n");
    }

    #[test]
    fn insert_line_multiline_with_blank_lines() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nepsilon\n").unwrap();

        insert_line(&mut doc, 1, "beta\ngamma").unwrap();

        assert_eq!(doc.lines.len(), 4);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "beta");
        assert_eq!(doc.lines[2].content.as_ref(), "gamma");
        assert_eq!(doc.lines[3].content.as_ref(), "epsilon");
        assert_eq!(doc.render(), b"alpha\nbeta\ngamma\nepsilon\n");
    }

    #[test]
    fn delete_line_removes_middle_line() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let original_hash = doc.lines[2].short_hash;

        delete_line(&mut doc, 1).unwrap();

        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "gamma");
        assert_eq!(doc.lines[1].short_hash, original_hash);
        assert_eq!(doc.render(), b"alpha\ngamma\n");
    }

    #[test]
    fn delete_last_remaining_line_produces_empty_document() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha").unwrap();

        delete_line(&mut doc, 0).unwrap();

        assert!(doc.lines.is_empty());
        assert_eq!(doc.render(), b"");
        assert!(!doc.trailing_newline);
    }

    #[test]
    fn delete_range_removes_multiple_lines() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let original_hash = doc.lines[3].short_hash;

        delete_range(&mut doc, 1, 2).unwrap();

        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].content.as_ref(), "alpha");
        assert_eq!(doc.lines[1].content.as_ref(), "delta");
        assert_eq!(doc.lines[1].short_hash, original_hash);
        assert_eq!(doc.render(), b"alpha\ndelta\n");
    }

    #[test]
    fn swap_lines_exchanges_contents_and_recomputes_numbers() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let beta_hash = doc.lines[1].short_hash;
        let delta_hash = doc.lines[3].short_hash;

        swap_lines(&mut doc, 1, 3).unwrap();

        assert_eq!(doc.render(), b"alpha\ndelta\ngamma\nbeta\n");
        assert_eq!(doc.lines[1].short_hash, delta_hash);
        assert_eq!(doc.lines[3].short_hash, beta_hash);
    }

    #[test]
    fn swap_lines_rejects_same_source_and_target() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let error = swap_lines(&mut doc, 1, 1).unwrap_err();
        assert!(matches!(
            error,
            HashlineError::PatchFailed { op_index: 0, .. }
        ));
    }

    #[test]
    fn move_line_after_target_adjusts_when_source_is_above_target() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let alpha_hash = doc.lines[0].short_hash;
        let beta_hash = doc.lines[1].short_hash;

        let inserted_at = move_line(&mut doc, 1, 3, false).unwrap();

        assert_eq!(inserted_at, 3);
        assert_eq!(doc.render(), b"alpha\ngamma\ndelta\nbeta\n");
        assert_eq!(doc.lines[0].short_hash, alpha_hash);
        assert_eq!(doc.lines[3].short_hash, beta_hash);
    }

    #[test]
    fn move_line_before_target_adjusts_when_source_is_above_target() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();

        let inserted_at = move_line(&mut doc, 1, 3, true).unwrap();

        assert_eq!(inserted_at, 2);
        assert_eq!(doc.render(), b"alpha\ngamma\nbeta\ndelta\n");
    }

    #[test]
    fn move_line_before_target_keeps_target_position_when_source_is_below() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();

        let inserted_at = move_line(&mut doc, 3, 1, true).unwrap();

        assert_eq!(inserted_at, 1);
        assert_eq!(doc.render(), b"alpha\ndelta\nbeta\ngamma\n");
    }

    #[test]
    fn move_line_rejects_same_source_and_target() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let error = move_line(&mut doc, 1, 1, true).unwrap_err();
        assert!(matches!(
            error,
            HashlineError::PatchFailed { op_index: 0, .. }
        ));
    }

    #[test]
    fn preserves_crlf_and_trailing_newline_flags_through_mutation() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\r\nbeta\r\n").unwrap();

        insert_line(&mut doc, 1, "middle").unwrap();

        assert_eq!(doc.newline, NewlineStyle::Crlf);
        assert!(doc.trailing_newline);
        assert_eq!(doc.render(), b"alpha\r\nmiddle\r\nbeta\r\n");
    }

    #[test]
    fn multiline_content_is_rejected() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        let error = replace_line(&mut doc, 0, "beta\ngamma").unwrap_err();
        assert!(matches!(error, HashlineError::MultiLineContentUnsupported));
    }

    #[test]
    fn split_content_lines_preserves_internal_blank_lines() {
        assert_eq!(
            split_content_lines("alpha\n\nbeta"),
            vec![
                Box::<str>::from("alpha"),
                Box::<str>::from(""),
                Box::<str>::from("beta")
            ]
        );
    }

    #[test]
    fn invalid_indices_are_rejected() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        let error = delete_line(&mut doc, 1).unwrap_err();
        assert!(matches!(
            error,
            HashlineError::MutationIndexOutOfBounds { index: 1, len: 1 }
        ));
    }

    #[test]
    fn invalid_range_is_rejected() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let error = replace_range_with_line(&mut doc, 1, 2, "gamma").unwrap_err();
        assert!(matches!(
            error,
            HashlineError::InvalidMutationRange {
                start: 1,
                end: 2,
                len: 2
            }
        ));
    }

    #[test]
    fn validate_single_line_content_rejects_carriage_return() {
        let error = validate_single_line_content("alpha\rbeta").unwrap_err();
        assert!(matches!(error, HashlineError::MultiLineContentUnsupported));
    }
}
