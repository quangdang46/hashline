#![allow(dead_code)]

use crate::document::{Document, ShortHashIndex, format_short_hash};
use crate::error::HashlineError;
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

pub fn parse_anchor(s: &str) -> Result<Anchor, HashlineError> {
    let normalized = normalize_anchor_input(s);

    if normalized.contains("..") {
        return Err(HashlineError::InvalidAnchor {
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

pub fn parse_range(s: &str) -> Result<RangeAnchor, HashlineError> {
    let normalized = normalize_anchor_input(s);
    let (left, right) = normalized
        .split_once("..")
        .ok_or_else(|| HashlineError::InvalidRange {
            range: s.trim().to_owned(),
        })?;

    if right.contains("..") {
        return Err(HashlineError::InvalidRange {
            range: s.trim().to_owned(),
        });
    }

    let start = parse_anchor(left).map_err(|_| HashlineError::InvalidRange {
        range: s.trim().to_owned(),
    })?;
    let end = parse_anchor(right).map_err(|_| HashlineError::InvalidRange {
        range: s.trim().to_owned(),
    })?;

    if !matches!(start, Anchor::LineHash { .. }) || !matches!(end, Anchor::LineHash { .. }) {
        return Err(HashlineError::InvalidRange {
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
) -> Result<ResolvedLine, HashlineError> {
    match anchor {
        Anchor::Hash { short } => resolve_unqualified(*short, doc, index),
        Anchor::LineHash { line, short } => resolve_qualified(*line, *short, doc, Some(index)),
    }
}

pub fn resolve_without_index(
    anchor: &Anchor,
    doc: &Document,
) -> Result<ResolvedLine, HashlineError> {
    match anchor {
        Anchor::Hash { short } => {
            let index = doc.build_index();
            resolve_unqualified(*short, doc, &index)
        }
        Anchor::LineHash { line, short } => match resolve_qualified(*line, *short, doc, None) {
            Ok(resolved) => Ok(resolved),
            Err(HashlineError::StaleAnchor { .. }) => {
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
) -> Result<(ResolvedLine, ResolvedLine), HashlineError> {
    let start = resolve(&range.start, doc, index)?;
    let end = resolve(&range.end, doc, index)?;

    if start.index > end.index {
        return Err(HashlineError::InvalidRange {
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
) -> Vec<Result<ResolvedLine, HashlineError>> {
    anchors
        .iter()
        .map(|anchor| resolve(anchor, doc, index))
        .collect()
}

fn resolve_unqualified(
    short: ShortHash,
    doc: &Document,
    index: &ShortHashIndex,
) -> Result<ResolvedLine, HashlineError> {
    let path = doc.path.display().to_string();
    let rendered_short = format_short_hash(short);
    match index[short as usize].as_slice() {
        [] => Err(HashlineError::HashNotFound {
            hash: rendered_short,
            path,
        }),
        [resolved_index] => Ok(ResolvedLine {
            index: *resolved_index,
            line_no: resolved_index + 1,
            short_hash: rendered_short,
        }),
        matches => Err(HashlineError::AmbiguousHash {
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
) -> Result<ResolvedLine, HashlineError> {
    let path = doc.path.display().to_string();
    let rendered_short = format_short_hash(short);
    let idx = line
        .checked_sub(1)
        .ok_or_else(|| HashlineError::InvalidAnchor {
            anchor: format!("{line}:{rendered_short}"),
        })?;

    let actual = doc
        .lines
        .get(idx)
        .ok_or_else(|| HashlineError::InvalidAnchor {
            anchor: format!("{line}:{rendered_short}"),
        })?;

    // Fast path: exact line+hash match.
    if actual.short_hash == short {
        return Ok(ResolvedLine {
            index: idx,
            line_no: line,
            short_hash: rendered_short,
        });
    }

    // Fuzzy relocation: when line+hash mismatch, look for the same hash
    // elsewhere in the file. This lets edits survive small line shifts
    // (e.g. a sibling `use` statement added at top, formatter inserting
    // a blank line, etc.) without forcing the agent to re-read.
    //
    // Logic (matches hashfile-mcp):
    //   - 1 unique match anywhere → silently relocate to it
    //   - N matches → relocate to the one closest to the requested line
    //     IF that closest match is within ±FUZZY_RELOCATE_RADIUS lines.
    //     Beyond that radius, the request is ambiguous enough that the
    //     agent should re-read.
    //
    // This only fires for QUALIFIED anchors (line:hash). Unqualified
    // anchors (just hash) already use the global index lookup.
    const FUZZY_RELOCATE_RADIUS: isize = 3;

    if let Some(index) = index {
        let candidates = &index[short as usize];
        let relocated = match candidates.as_slice() {
            [] => None,
            [single] => Some(*single),
            many => {
                // Pick the candidate closest to the requested line.
                let target = idx as isize;
                let closest = many
                    .iter()
                    .min_by_key(|&&c| (c as isize - target).abs())
                    .copied()
                    .expect("non-empty");
                let dist = (closest as isize - target).abs();
                if dist <= FUZZY_RELOCATE_RADIUS {
                    Some(closest)
                } else {
                    None
                }
            }
        };

        if let Some(new_idx) = relocated {
            return Ok(ResolvedLine {
                index: new_idx,
                line_no: new_idx + 1,
                short_hash: rendered_short,
            });
        }
    }

    // Build a rich context block showing the stale line with its NEW hash
    // plus ±2 lines of surrounding context, so the agent can copy a fresh
    // anchor verbatim without re-reading the whole file.
    //
    // Format (matches pi-hashline-edit's >>> convention):
    //
    //     3:f1|  context
    //     4:b3|  context
    //   >>> 5:7e|  stale line with current hash
    //     6:c2|  context
    //
    // Plus, if the requested hash exists elsewhere, list those lines too.
    const CONTEXT_RADIUS: usize = 2;
    let lo = idx.saturating_sub(CONTEXT_RADIUS);
    let hi = (idx + CONTEXT_RADIUS).min(doc.lines.len().saturating_sub(1));
    let mut context = String::new();
    context.push('\n');
    for i in lo..=hi {
        let line_record = &doc.lines[i];
        let prefix = if i == idx { ">>> " } else { "    " };
        let line_no = i + 1;
        let hash = format_short_hash(line_record.short_hash);
        // Truncate very long lines so the error stays readable.
        let content: &str = &line_record.content;
        let display: std::borrow::Cow<'_, str> = if content.len() > 80 {
            std::borrow::Cow::Owned(format!("{}…", &content[..80]))
        } else {
            std::borrow::Cow::Borrowed(content)
        };
        context.push_str(&format!("{prefix}{line_no}:{hash}|{display}\n"));
    }

    // If the requested hash exists elsewhere (beyond fuzzy-relocation radius),
    // tell the agent where to look.
    if let Some(index) = index {
        let elsewhere = &index[short as usize];
        if !elsewhere.is_empty() {
            let lines = elsewhere
                .iter()
                .map(|idx| (idx + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            context.push_str(&format!(
                "(hash {rendered_short} also at line(s) {lines})\n"
            ));
        }
    }
    let relocated_suffix = context;
    Err(HashlineError::StaleAnchor {
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

fn parse_short_hash(short: &str, original: &str) -> Result<ShortHash, HashlineError> {
    if short.len() == 2 && short.chars().all(|ch| ch.is_ascii_hexdigit()) {
        u8::from_str_radix(short, 16).map_err(|_| HashlineError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })
    } else {
        Err(HashlineError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })
    }
}

fn parse_line_number(raw: &str, original: &str) -> Result<usize, HashlineError> {
    let line = raw
        .parse::<usize>()
        .map_err(|_| HashlineError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        })?;

    if line == 0 {
        return Err(HashlineError::InvalidAnchor {
            anchor: original.trim().to_owned(),
        });
    }

    Ok(line)
}

/// A region identified by content queries rather than hashed anchors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionPattern {
    pub start_line: usize,
    pub end_line: usize,
}

/// Resolve a content query to a unique line number in the document.
///
/// The query is a plain substring match (not regex). Returns an error if
/// the query matches zero lines or more than one line.
pub fn find_line_by_query(
    doc: &Document,
    query: &str,
) -> Result<usize, HashlineError> {
    let path = doc.path.display().to_string();
    let matches: Vec<usize> = doc
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.content.contains(query))
        .map(|(idx, _)| idx + 1) // 1-indexed line numbers
        .collect();

    match matches.len() {
        0 => Err(HashlineError::QueryNotFound {
            query: query.to_string(),
            path,
        }),
        1 => Ok(matches[0]),
        n => {
            let lines_str = matches
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(HashlineError::AmbiguousQuery {
                query: query.to_string(),
                count: n,
                lines: lines_str,
                path,
            })
        }
    }
}

/// Convert a pair of (optional) content queries into a `RegionPattern`.
///
/// * If neither query is set, returns `None`.
/// * If `start_query` is set, it is resolved to a unique line number.
/// * If `end_query` is set, it is resolved to a unique line number;
///   otherwise `end = start`.
/// * Validates that the range is within the sane limit (10,000 lines).
pub fn resolve_query_region(
    doc: &Document,
    start_query: Option<&str>,
    end_query: Option<&str>,
) -> Result<Option<RegionPattern>, HashlineError> {
    let Some(start_query) = start_query else {
        return Ok(None);
    };

    let start_line = find_line_by_query(doc, start_query)?;
    let end_line = match end_query {
        Some(q) => find_line_by_query(doc, q)?,
        None => start_line,
    };

    if start_line > end_line {
        return Err(HashlineError::InvalidRange {
            range: format!("query start (line {start_line}) after query end (line {end_line})"),
        });
    }

    let count = end_line - start_line + 1;
    const MAX_QUERY_RANGE: usize = 10_000;
    if count > MAX_QUERY_RANGE {
        return Err(HashlineError::QueryRangeTooLarge {
            count,
            max: MAX_QUERY_RANGE,
        });
    }

    Ok(Some(RegionPattern { start_line, end_line }))
}

/// Try to parse a simple "line:hexhash" anchor.
/// Returns `(0-indexed line_number, short_hash)` for LineHash anchors.
/// Returns `None` for range anchors, raw hashes, or invalid formats.
pub fn try_parse_line_anchor(anchor: &str) -> Option<(usize, crate::hash::ShortHash)> {
    let normalized = anchor.trim();
    if normalized.contains("..") {
        return None;
    }
    parse_anchor(normalized).ok().and_then(|a| match a {
        Anchor::LineHash { line, short } => line.checked_sub(1).map(|zb| (zb, short)),
        _ => None,
    })
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
        Anchor, ResolvedLine, looks_like_range_anchor, parse_anchor, parse_range, resolve,
        resolve_all, resolve_range,
    };
    use crate::document::Document;
    use crate::error::HashlineError;
    use crate::hash::format_short_hash;
    use anyhow::{Result, anyhow};
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
            Err(HashlineError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_invalid_hash_non_hex_fails() {
        assert!(matches!(
            parse_anchor("zz"),
            Err(HashlineError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_line_number_zero_fails() {
        assert!(matches!(
            parse_anchor("0:aa"),
            Err(HashlineError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_parse_line_number_negative_fails() {
        assert!(matches!(
            parse_anchor("-1:aa"),
            Err(HashlineError::InvalidAnchor { .. })
        ));
    }

    #[test]
    fn test_resolve_unqualified_not_found() -> Result<()> {
        let doc = sample_doc()?;
        let index = doc.build_index();
        let error = must_err(resolve(&Anchor::Hash { short: 0xff }, &doc, &index))?;

        assert!(matches!(error, HashlineError::HashNotFound { .. }));
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

        assert!(matches!(error, HashlineError::AmbiguousHash { .. }));
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

        assert!(matches!(error, HashlineError::StaleAnchor { .. }));
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_fuzzy_relocates_within_3_lines() -> Result<()> {
        // Phase 2: when line+hash mismatch but the hash exists nearby
        // (within ±3 lines), silently relocate to the unique closest match.
        // This lets edits survive small line shifts.
        let doc = sample_doc()?;
        let index = doc.build_index();
        let relocated_hash = doc.lines[0].short_hash;
        // Request line 2 with line-1's hash → distance is 1, should relocate
        let resolved = must(resolve(
            &Anchor::LineHash {
                line: 2,
                short: relocated_hash,
            },
            &doc,
            &index,
        ))?;
        assert_eq!(resolved.index, 0, "should fuzzy-relocate to line 1");
        assert_eq!(resolved.line_no, 1);
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_unique_hash_relocates_anywhere() -> Result<()> {
        // When the requested hash exists at exactly ONE line in the file,
        // we relocate unconditionally — the line number was just a hint,
        // and the hash is the canonical identifier. This matches the
        // behavior of hashfile-mcp.
        let doc = far_doc()?;
        let index = doc.build_index();
        let line1_hash = doc.lines[0].short_hash;
        // Request line 10 with line-1's hash — unique match anywhere → relocate
        let resolved = must(resolve(
            &Anchor::LineHash {
                line: 10,
                short: line1_hash,
            },
            &doc,
            &index,
        ))?;
        assert_eq!(resolved.index, 0, "should relocate to line 1");
        Ok(())
    }

    #[test]
    fn test_resolve_qualified_stale_when_hash_absent() -> Result<()> {
        // When the requested hash exists nowhere in the file, return
        // StaleAnchor with a hint that the hash is gone.
        let doc = sample_doc()?;
        let index = doc.build_index();
        // 0xff is unlikely to match any of "alpha"/"beta"/"gamma".
        // (sample_doc uses lines that don't hash to 0xff — verified
        // by `test_resolve_unqualified_not_found` above.)
        let error = must_err(resolve(
            &Anchor::LineHash {
                line: 2,
                short: 0xff,
            },
            &doc,
            &index,
        ))?;
        assert!(matches!(error, HashlineError::StaleAnchor { .. }));
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

        assert!(matches!(error, HashlineError::InvalidAnchor { .. }));
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
        assert!(matches!(error, HashlineError::InvalidRange { .. }));
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
            Err(HashlineError::AmbiguousHash { .. })
        ));
        assert!(matches!(
            results[1],
            Err(HashlineError::HashNotFound { .. })
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

    fn far_doc() -> Result<Document> {
        // 10 distinct lines: line-1 hash is at index 0, far from index 9.
        // Used to test that fuzzy relocation refuses when distance > 3.
        let content = (1..=10)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        must(Document::from_str(Path::new("demo.txt"), &content))
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
