#![allow(dead_code)]

use crate::document::{Document, LineRecord};
use crate::error::LinehashError;
use crate::hash;

pub fn validate_single_line_content(content: &str) -> Result<(), LinehashError> {
    if content.contains(['\n', '\r']) {
        Err(LinehashError::MultiLineContentUnsupported)
    } else {
        Ok(())
    }
}

pub fn split_content_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }

    let lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub fn replace_line(doc: &mut Document, index: usize, content: &str) -> Result<(), LinehashError> {
    validate_single_line_content(content)?;
    ensure_index(doc, index)?;

    let old_len = doc.lines[index].content.len();
    if doc.lines[index].content != content {
        doc.lines[index].content = content.to_owned();
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
) -> Result<(), LinehashError> {
    validate_single_line_content(content)?;
    replace_range(doc, start, end, content)
}

pub fn replace_range(
    doc: &mut Document,
    start: usize,
    end: usize,
    content: &str,
) -> Result<(), LinehashError> {
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

pub fn insert_line(doc: &mut Document, index: usize, content: &str) -> Result<(), LinehashError> {
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

pub fn delete_line(doc: &mut Document, index: usize) -> Result<(), LinehashError> {
    ensure_index(doc, index)?;

    let removed_len = doc.lines[index].content.len();
    doc.lines.remove(index);
    doc.content_len -= removed_len;
    Ok(())
}

pub fn delete_range(doc: &mut Document, start: usize, end: usize) -> Result<(), LinehashError> {
    ensure_range(doc, start, end)?;

    let removed_len: usize = doc.lines[start..=end]
        .iter()
        .map(|line| line.content.len())
        .sum();
    doc.lines.drain(start..=end);
    doc.content_len -= removed_len;
    Ok(())
}

pub fn swap_lines(doc: &mut Document, left: usize, right: usize) -> Result<(), LinehashError> {
    ensure_index(doc, left)?;
    ensure_index(doc, right)?;

    if left == right {
        return Err(LinehashError::PatchFailed {
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
) -> Result<usize, LinehashError> {
    ensure_index(doc, source)?;
    ensure_index(doc, target)?;

    if source == target {
        return Err(LinehashError::PatchFailed {
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
        content: content.to_owned(),
        short_hash: hash::short_from_full(full_hash),
    }
}

fn ensure_index(doc: &Document, index: usize) -> Result<(), LinehashError> {
    if index < doc.lines.len() {
        Ok(())
    } else {
        Err(LinehashError::MutationIndexOutOfBounds {
            index,
            len: doc.lines.len(),
        })
    }
}

fn ensure_insert_index(doc: &Document, index: usize) -> Result<(), LinehashError> {
    if index <= doc.lines.len() {
        Ok(())
    } else {
        Err(LinehashError::MutationIndexOutOfBounds {
            index,
            len: doc.lines.len(),
        })
    }
}

fn ensure_range(doc: &Document, start: usize, end: usize) -> Result<(), LinehashError> {
    if start <= end && end < doc.lines.len() {
        Ok(())
    } else {
        Err(LinehashError::InvalidMutationRange {
            start,
            end,
            len: doc.lines.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        delete_line, delete_range, insert_line, move_line, replace_line, replace_range,
        replace_range_with_line, split_content_lines, swap_lines, validate_single_line_content,
    };
    use crate::document::{Document, NewlineStyle};
    use crate::error::LinehashError;
    use std::path::Path;

    #[test]
    fn replace_line_recomputes_hashes_and_preserves_document_flags() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let original_newline = doc.newline;
        let original_trailing_newline = doc.trailing_newline;

        replace_line(&mut doc, 1, "gamma").unwrap();

        assert_eq!(doc.lines[1].content, "gamma");
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
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "merged");
        assert_eq!(doc.lines[2].content, "delta");
        assert_eq!(doc.render(), b"alpha\nmerged\ndelta\n");
    }

    #[test]
    fn replace_range_expands_to_multiple_lines() {
        let mut doc =
            Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();

        replace_range(&mut doc, 1, 2, "left\nmiddle\nright").unwrap();

        assert_eq!(doc.lines.len(), 5);
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "left");
        assert_eq!(doc.lines[2].content, "middle");
        assert_eq!(doc.lines[3].content, "right");
        assert_eq!(doc.lines[4].content, "delta");
        assert_eq!(doc.render(), b"alpha\nleft\nmiddle\nright\ndelta\n");
    }

    #[test]
    fn insert_line_at_index_renumbers_following_lines() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\ngamma\n").unwrap();
        let original_hash = doc.lines[1].short_hash;

        insert_line(&mut doc, 1, "beta").unwrap();

        assert_eq!(doc.lines.len(), 3);
        assert_eq!(doc.lines[1].content, "beta");
        assert_eq!(doc.lines[2].content, "gamma");
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
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "beta");
        assert_eq!(doc.lines[2].content, "gamma");
        assert_eq!(doc.lines[3].content, "delta");
        assert_eq!(doc.render(), b"alpha\nbeta\ngamma\ndelta\n");
    }

    #[test]
    fn insert_line_multiline_with_blank_lines() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nepsilon\n").unwrap();

        insert_line(&mut doc, 1, "beta\ngamma").unwrap();

        assert_eq!(doc.lines.len(), 4);
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "beta");
        assert_eq!(doc.lines[2].content, "gamma");
        assert_eq!(doc.lines[3].content, "epsilon");
        assert_eq!(doc.render(), b"alpha\nbeta\ngamma\nepsilon\n");
    }

    #[test]
    fn delete_line_removes_middle_line() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let original_hash = doc.lines[2].short_hash;

        delete_line(&mut doc, 1).unwrap();

        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "gamma");
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
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "delta");
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
            LinehashError::PatchFailed { op_index: 0, .. }
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
            LinehashError::PatchFailed { op_index: 0, .. }
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
        assert!(matches!(error, LinehashError::MultiLineContentUnsupported));
    }

    #[test]
    fn split_content_lines_preserves_internal_blank_lines() {
        assert_eq!(
            split_content_lines("alpha\n\nbeta"),
            vec!["alpha".to_owned(), String::new(), "beta".to_owned()]
        );
    }

    #[test]
    fn invalid_indices_are_rejected() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        let error = delete_line(&mut doc, 1).unwrap_err();
        assert!(matches!(
            error,
            LinehashError::MutationIndexOutOfBounds { index: 1, len: 1 }
        ));
    }

    #[test]
    fn invalid_range_is_rejected() {
        let mut doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let error = replace_range_with_line(&mut doc, 1, 2, "gamma").unwrap_err();
        assert!(matches!(
            error,
            LinehashError::InvalidMutationRange {
                start: 1,
                end: 2,
                len: 2
            }
        ));
    }

    #[test]
    fn validate_single_line_content_rejects_carriage_return() {
        let error = validate_single_line_content("alpha\rbeta").unwrap_err();
        assert!(matches!(error, LinehashError::MultiLineContentUnsupported));
    }
}
