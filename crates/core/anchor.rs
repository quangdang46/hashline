#![allow(dead_code)]

use crate::document::{format_short_hash, Document, ShortHashIndex};
use crate::error::LinehashError;
use crate::hash::ShortHash;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Anchor {
    Hash { short: ShortHash },
    LineHash { line: usize, short: ShortHash },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeAnchor {
    pub start: Anchor,
    pub end: Anchor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedLine {
    pub index: usize,
    pub line_no: usize,
    pub short_hash: String,
}

pub fn parse_anchor(s: &str) -> Result<Anchor, LinehashError> {
    let normalized = normalize_anchor_input(s);

    if normalized.contains("..") {
        return Err(LinehashError::InvalidAnchor {
            anchor: s.trim().to_owned(),
        });
    }

    if let Some((line, short)) = normalized.split_once(':') {
        let line = parse_line_number(line, s)?;
        let short = parse_short_hash(short, s)?;
        return Ok(Anchor::LineHash { line, short });
    }

    let short = parse_short_hash(&normalized, s)?;
    Ok(Anchor::Hash { short })
}

pub fn parse_range(s: &str) -> Result<RangeAnchor, LinehashError> {
    let normalized = normalize_anchor_input(s);
    let (left, right) = normalized
        .split_once("..")
        .ok_or_else(|| LinehashError::InvalidRange {
            range: s.trim().to_owned(),
        })?;

    if right.contains("..") {
        return Err(LinehashError::InvalidRange {
            range: s.trim().to_owned(),
        });
    }

    let start = parse_anchor(left).map_err(|_| LinehashError::InvalidRange {
        range: s.trim().to_owned(),
    })?;
    let end = parse_anchor(right).map_err(|_| LinehashError::InvalidRange {
        range: s.trim().to_owned(),
    })?;

    if !matches!(start, Anchor::LineHash { .. }) || !matches!(end, Anchor::LineHash { .. }) {
        return Err(LinehashError::InvalidRange {
            range: s.trim().to_owned(),
        });
    }

    Ok(RangeAnchor { start, end })
}

pub fn looks_like_range_anchor(s: &str) -> bool {
    let normalized = normalize_anchor_input(s);
    if normalized.contains("..") {
        return true;
    }

    normalized
        .split_once('-')
        .is_some_and(|(left, right)| parse_anchor(left).is_ok() && parse_anchor(right).is_ok())
}

pub fn resolve(
    anchor: &Anchor,
    doc: &Document,
    index: &ShortHashIndex,
) -> Result<ResolvedLine, LinehashError> {
    match anchor {
        Anchor::Hash { short } => resolve_unqualified(*short, doc, index),
        Anchor::LineHash { line, short } => resolve_qualified(*line, *short, doc, Some(index)),
    }
}

pub fn resolve_without_index(
    anchor: &Anchor,
    doc: &Document,
) -> Result<ResolvedLine, LinehashError> {
    match anchor {
        Anchor::Hash { short } => {
            let index = doc.build_index();
            resolve_unqualified(*short, doc, &index)
        }
        Anchor::LineHash { line, short } => match resolve_qualified(*line, *short, doc, None) {
            Ok(resolved) => Ok(resolved),
            Err(LinehashError::StaleAnchor { .. }) => {
                let index = doc.build_index();
                resolve_qualified(*line, *short, doc, Some(&index))
            }
            Err(error) => Err(error),
        },
    }
}

pub fn resolve_range(
    range: &RangeAnchor,
    doc: &Document,
    index: &ShortHashIndex,
) -> Result<(ResolvedLine, ResolvedLine), LinehashError> {
    let start = resolve(&range.start, doc, index)?;
    let end = resolve(&range.end, doc, index)?;

    if start.index > end.index {
        return Err(LinehashError::InvalidRange {
            range: format!(
                "{}..{}",
                display_anchor(&range.start),
                display_anchor(&range.end)
            ),
        });
    }

    Ok((start, end))
}

pub fn resolve_all(
    anchors: &[Anchor],
    doc: &Document,
    index: &ShortHashIndex,
) -> Vec<Result<ResolvedLine, LinehashError>> {
    anchors
        .iter()
        .map(|anchor| resolve(anchor, doc, index))
        .collect()
}

fn resolve_unqualified(
    short: ShortHash,
    doc: &Document,
    index: &ShortHashIndex,
) -> Result<ResolvedLine, LinehashError> {
    let path = doc.path.display().to_string();
    let rendered_short = format_short_hash(short);
    match index[short as usize].as_slice() {
        [] => Err(LinehashError::HashNotFound {
            hash: rendered_short,
            path,
        }),
        [resolved_index] => Ok(ResolvedLine {
            index: *resolved_index,
            line_no: resolved_index + 1,
            short_hash: rendered_short,
        }),
        matches => Err(LinehashError::AmbiguousHash {
            hash: rendered_short,
            count: matches.len(),
            lines: matches
                .iter()
                .map(|idx| (idx + 1).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            path,
        }),
    }
}

fn resolve_qualified(
    line: usize,
    short: ShortHash,
    doc: &Document,
    index: Option<&ShortHashIndex>,
) -> Result<ResolvedLine, LinehashError> {
    let path = doc.path.display().to_string();
    let rendered_short = format_short_hash(short);
    let idx = line
        .checked_sub(1)
        .ok_or_else(|| LinehashError::InvalidAnchor {
            anchor: format!("{line}:{rendered_short}"),
        })?;

    let actual = doc
        .lines
        .get(idx)
        .ok_or_else(|| LinehashError::InvalidAnchor {
            anchor: format!("{line}:{rendered_short}"),
        })?;

    if actual.short_hash == short {
        return Ok(ResolvedLine {
            index: idx,
            line_no: line,
            short_hash: rendered_short,
        });
    }

    let relocated_suffix = if let Some(index) = index {
        if !index[short as usize].is_empty() {
            let lines = index[short as usize]
                .iter()
                .map(|idx| (idx + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("; hash still exists at line(s) {lines}")
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    Err(LinehashError::StaleAnchor {
        anchor: format!("{line}:{rendered_short}").into_boxed_str(),
        line,
        expected: rendered_short.into_boxed_str(),
        actual: format_short_hash(actual.short_hash).into_boxed_str(),
        path: path.into_boxed_str(),
        relocated_suffix: relocated_suffix.into_boxed_str(),
    })
}

fn normalize_anchor_input(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn parse_short_hash(short: &str, original: &str) -> Result<ShortHash, LinehashError> {
    if short.len() == 2 && short.chars().all(|ch| ch.is_ascii_hexdigit()) {
        u8::from_str_radix(short, 16).map_err(|_| LinehashError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })
    } else {
        Err(LinehashError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })
    }
}

fn parse_line_number(raw: &str, original: &str) -> Result<usize, LinehashError> {
    let line = raw
        .parse::<usize>()
        .map_err(|_| LinehashError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })?;

    if line == 0 {
        return Err(LinehashError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        });
    }

    Ok(line)
}

fn display_anchor(anchor: &Anchor) -> String {
    match anchor {
        Anchor::Hash { short } => format_short_hash(*short),
        Anchor::LineHash { line, short } => format!("{line}:{}", format_short_hash(*short)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        looks_like_range_anchor, parse_anchor, parse_range, resolve, resolve_all, resolve_range,
        Anchor, ResolvedLine,
    };
    use crate::document::Document;
    use crate::error::LinehashError;
    use crate::hash::format_short_hash;
    use anyhow::{anyhow, Result};
    use std::fmt::Display;
    use std::path::Path;

    fn must<T, E: Display>(result: std::result::Result<T, E>) -> Result<T> {
        result.map_err(|error| anyhow!("{error}"))
    }

    fn must_err<T, E: Display>(result: std::result::Result<T, E>) -> Result<E> {
        match result {
            Ok(_) => Err(anyhow!("expected error")),
            Err(error) => Ok(error),
        }
    }

    #[test]
    fn test_parse_unqualified_lowercase() -> Result<()> {
        assert_eq!(must(parse_anchor("f1"))?, Anchor::Hash { short: 0xf1 });
        Ok(())
    }

    #[test]
    fn test_parse_unqualified_uppercase_normalizes() -> Result<()> {
        assert_eq!(must(parse_anchor("F1"))?, Anchor::Hash { short: 0xf1 });
        Ok(())
    }

    #[test]
    fn test_parse_qualified_basic() -> Result<()> {
        assert_eq!(
            must(parse_anchor("2:f1"))?,
            Anchor::LineHash {
                line: 2,
                short: 0xf1
            }
        );
        Ok(())
    }

    #[test]
    fn test_parse_qualified_uppercase_normalizes() -> Result<()> {
        assert_eq!(
            must(parse_anchor("2:F1"))?,
            Anchor::LineHash {
                line: 2,
                short: 0xf1
            }
        );
        Ok(())
    }

    #[test]
    fn test_parse_range_basic() -> Result<()> {
        let range = must(parse_range("2:f1..4:9c"))?;
        assert_eq!(
            range.start,
            Anchor::LineHash {
                line: 2,
                short: 0xf1
            }
        );
        assert_eq!(
            range.end,
            Anchor::LineHash {
                line: 4,
                short: 0x9c
            }
        );
        Ok(())
    }

    #[test]
    fn test_range_detection_accepts_dash_separator_for_hint_routing() {
        assert!(looks_like_range_anchor("2:f1-4:9c"));
        assert!(looks_like_range_anchor("2:f1..4:9c"));
        assert!(!looks_like_range_anchor("2:f1"));
        assert!(!looks_like_range_anchor("-1:aa"));
    }

    #[test]
    fn test_parse_invalid_hash_length_3_chars_fails() {
        assert!(matches!(
            parse_anchor("abc"),
            Err(LinehashError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_invalid_hash_non_hex_fails() {
        assert!(matches!(
            parse_anchor("zz"),
            Err(LinehashError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_line_number_zero_fails() {
        assert!(matches!(
            parse_anchor("0:aa"),
            Err(LinehashError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_line_number_negative_fails() {
        assert!(matches!(
            parse_anchor("-1:aa"),
            Err(LinehashError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_resolve_unqualified_not_found() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let error = must_err(resolve(&Anchor::Hash { short: 0xff }, &doc, &index))?;

        assert!(matches!(error, LinehashError::HashNotFound { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_unqualified_single_match() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let short = doc.lines[1].short_hash;

        assert_eq!(
            must(resolve(&Anchor::Hash { short }, &doc, &index))?,
            ResolvedLine {
                index: 1,
                line_no: 2,
                short_hash: format_short_hash(short)
            }
        );
        Ok(())
    }

    #[test]
    fn test_resolve_unqualified_ambiguous() -> Result<()> {
        let doc = collision_doc()?;
        let index = doc.build_index();
        let short = doc.lines[0].short_hash;
        let error = must_err(resolve(&Anchor::Hash { short }, &doc, &index))?;

        assert!(matches!(error, LinehashError::AmbiguousHash { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_match() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let short = doc.lines[1].short_hash;

        assert_eq!(
            must(resolve(&Anchor::LineHash { line: 2, short }, &doc, &index))?,
            ResolvedLine {
                index: 1,
                line_no: 2,
                short_hash: format_short_hash(short)
            }
        );
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_stale() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let error = must_err(resolve(
            &Anchor::LineHash {
                line: 2,
                short: 0xff,
            },
            &doc,
            &index,
        ))?;

        assert!(matches!(error, LinehashError::StaleAnchor { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_stale_mentions_relocated_hash_when_present() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let relocated_hash = doc.lines[0].short_hash;
        let error = must_err(resolve(
            &Anchor::LineHash {
                line: 2,
                short: relocated_hash,
            },
            &doc,
            &index,
        ))?;

        let rendered = error.to_string();
        assert!(matches!(error, LinehashError::StaleAnchor { .. }));
        assert!(rendered.contains("hash still exists at line(s) 1"));
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_out_of_range_line() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let error = must_err(resolve(
            &Anchor::LineHash {
                line: 99,
                short: 0xaa,
            },
            &doc,
            &index,
        ))?;

        assert!(matches!(error, LinehashError::InvalidAnchor { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_range_valid() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let start = format!("1:{}", format_short_hash(doc.lines[0].short_hash));
        let end = format!("3:{}", format_short_hash(doc.lines[2].short_hash));
        let range = must(parse_range(&format!("{start}..{end}")))?;

        let (resolved_start, resolved_end) = must(resolve_range(&range, &doc, &index))?;
        assert_eq!(resolved_start.index, 0);
        assert_eq!(resolved_end.index, 2);
        Ok(())
    }

    #[test]
    fn test_resolve_range_start_after_end_fails() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let start = format!("3:{}", format_short_hash(doc.lines[2].short_hash));
        let end = format!("1:{}", format_short_hash(doc.lines[0].short_hash));
        let range = must(parse_range(&format!("{start}..{end}")))?;

        let error = must_err(resolve_range(&range, &doc, &index))?;
        assert!(matches!(error, LinehashError::InvalidRange { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_all_collects_all_errors() -> Result<()> {
        let doc = collision_doc()?;
        let index = doc.build_index();
        let results = resolve_all(
            &[
                Anchor::Hash {
                    short: doc.lines[0].short_hash,
                },
                Anchor::Hash { short: 0xff },
            ],
            &doc,
            &index,
        );

        assert!(matches!(
            results[0],
            Err(LinehashError::AmbiguousHash { .. })
        ));
        assert!(matches!(
            results[1],
            Err(LinehashError::HashNotFound { .. })
        ));
        Ok(())
    }

    #[test]
    fn test_resolve_all_all_success() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let results = resolve_all(
            &[
                Anchor::LineHash {
                    line: 1,
                    short: doc.lines[0].short_hash,
                },
                Anchor::LineHash {
                    line: 2,
                    short: doc.lines[1].short_hash,
                },
            ],
            &doc,
            &index,
        );

        assert!(results.iter().all(|result| result.is_ok()));
        Ok(())
    }

    fn sample_doc() -> Result<Document> {
        must(Document::from_str(
            Path::new("demo.txt"),
            "alpha\nbeta\ngamma\n",
        ))
    }

    fn collision_doc() -> Result<Document> {
        for i in 0..10_000 {
            let left = format!("line-{i}");
            for j in (i + 1)..10_000 {
                let right = format!("line-{j}");
                let doc = must(Document::from_str(
                    Path::new("demo.txt"),
                    &format!("{left}\n{right}\n"),
                ))?;
                if doc.lines[0].short_hash == doc.lines[1].short_hash {
                    return Ok(doc);
                }
            }
        }
        Err(anyhow!("failed to find a collision doc"))
    }
}
