//! Expand deferred block edits (`replace_block N:` / `delete_block N` /
//! `insert_after_block N:`) into concrete inserts + deletes.
//!
//! The hashline parser cannot expand a block edit on its own — the line span is
//! unknown until file text + path (=> language) are available. This transform
//! runs at every apply/preview boundary that has text: it calls the injected
//! [`BlockResolver`] to resolve each block's `[start, end]` span, then emits the
//! exact same edits the concrete form produces in the parser: `replace
//! start.=end:` inserts + deletes for a replace, a pure range delete for a
//! delete, and plain `after_anchor` inserts at `end` for an insert-after. After
//! it runs, no `block` edits remain, so the applier (and recovery) only ever see
//! resolved edits.

use crate::types::{
    Anchor, BlockMode, BlockResolver, BlockResolverRequest, BlockSpan, Cursor, Edit, InsertMode,
};

/// Why [`resolve_block_edits`] could not expand a deferred block edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockResolveError {
    /// The block could not be resolved; the string is the human-facing reason.
    Unresolved(String),
    /// The op carried an anchor hash that does not match the line it names.
    /// Kept distinct from `Unresolved` so callers can surface a `StaleAnchor`
    /// (and attempt snapshot recovery) rather than a generic block failure.
    StaleAnchor {
        line: usize,
        expected: u8,
        actual: u8,
    },
}

impl std::fmt::Display for BlockResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unresolved(msg) => f.write_str(msg),
            Self::StaleAnchor {
                line,
                expected,
                actual,
            } => write!(
                f,
                "line {line} content changed since last read (expected hash {}, got {})",
                crate::hash::format_short_hash(*expected),
                crate::hash::format_short_hash(*actual)
            ),
        }
    }
}

/// Returns `true` when at least one edit is a deferred block edit.
pub fn has_block_edit(edits: &[Edit]) -> bool {
    edits.iter().any(|e| matches!(e, Edit::Block { .. }))
}

/// Resolve every deferred block edit in `edits` against `text` (parsed as the
/// language inferred from `path`). Non-block edits pass through untouched.
///
/// Returns a fresh edit list with no `Block` variants. The fast path returns
/// the input unchanged when there is nothing to resolve.
///
/// # Errors
///
/// Returns an error when:
/// - A `replace_block` or `delete_block` cannot be resolved (no resolver
///   available or the resolver returns `None`).
/// - A resolver returns a single-line span (start == end), which means the
///   anchor landed on a bare statement rather than the opening line of a
///   multi-line construct.
///
/// `insert_after_block` never errors: when unresolvable it is degraded to a
/// plain `insert after N:`.
pub fn resolve_block_edits(
    edits: &[Edit],
    text: &str,
    path: &str,
    resolver: Option<&dyn BlockResolver>,
) -> Result<Vec<Edit>, BlockResolveError> {
    if !has_block_edit(edits) {
        return Ok(edits.to_vec());
    }

    // Hash anchors are checked against this snapshot's per-line hashes, using
    // the same position-seeded scheme `read` reports (so symbol-only lines
    // validate too). Done before lowering, because the lowered edits no longer
    // carry the anchor the caller referenced.
    let anchor_hashes: Option<Vec<u8>> = edits
        .iter()
        .any(|e| {
            matches!(
                e,
                Edit::Block {
                    expected_hash: Some(_),
                    ..
                }
            )
        })
        .then(|| {
            let lines: Vec<&str> = if text.is_empty() {
                Vec::new()
            } else {
                text.split('\n').collect()
            };
            lines
                .iter()
                .enumerate()
                .map(|(i, line)| crate::hash::short_hash_value_indexed(line, i + 1))
                .collect()
        });

    let mut resolved: Vec<Edit> = Vec::with_capacity(edits.len());
    let mut synth_index = 0usize;

    for edit in edits {
        let Edit::Block {
            anchor,
            payloads,
            line_num,
            mode,
            expected_hash,
            ..
        } = edit
        else {
            resolved.push(edit.clone());
            continue;
        };

        if let (Some(expected), Some(hashes)) = (expected_hash, anchor_hashes.as_ref()) {
            let idx = anchor.line.wrapping_sub(1);
            if let Some(actual) = hashes.get(idx) {
                if *expected != *actual {
                    return Err(BlockResolveError::StaleAnchor {
                        line: anchor.line,
                        expected: *expected,
                        actual: *actual,
                    });
                }
            }
        }

        // Determine which concrete op this block edit corresponds to.
        let op = match mode {
            Some(BlockMode::InsertAfter) => "insert_after",
            Some(BlockMode::InsertBefore) => "insert_before",
            None if payloads.is_empty() => "delete",
            None => "replace",
        };

        // Attempt to resolve the block anchor.
        let span = resolver.and_then(|r| {
            r.resolve(&BlockResolverRequest {
                path: path.to_string(),
                text: text.to_string(),
                line: anchor.line,
            })
        });

        match span {
            None => {
                // `insert_after_block N:` is degraded: lower to a plain
                // `insert after N:` instead of failing the patch.
                if op == "insert_after" {
                    for payload in payloads {
                        resolved.push(Edit::Insert {
                            cursor: Cursor::AfterAnchor(Anchor { line: anchor.line }),
                            text: payload.clone(),
                            line_num: *line_num,
                            index: synth_index,
                            mode: None,
                            block_start: None,
                            expected_hash: None,
                        });
                        synth_index += 1;
                    }
                    continue;
                }

                // `insert_before_block N:` is similarly degraded.
                if op == "insert_before" {
                    for payload in payloads {
                        resolved.push(Edit::Insert {
                            cursor: Cursor::BeforeAnchor(Anchor { line: anchor.line }),
                            text: payload.clone(),
                            line_num: *line_num,
                            index: synth_index,
                            mode: None,
                            block_start: None,
                            expected_hash: None,
                        });
                        synth_index += 1;
                    }
                    continue;
                }

                // `replace_block` / `delete_block` with no resolution: fail.
                let msg = if resolver.is_none() {
                    format!(
                        "line {}: SWAP.BLK/DEL.BLK/INS.BLK.POST/INS.BLK.PRE are not available here \
                         (no block resolver configured). Use a concrete line range.",
                        line_num
                    )
                } else {
                    format!(
                        "line {}: cannot resolve block anchor for `{}` on line {} — \
                         unsupported language, blank/closer line, or parse error. \
                         Use a concrete line range.",
                        line_num, op, anchor.line
                    )
                };
                return Err(BlockResolveError::Unresolved(msg));
            }
            Some(BlockSpan { start, end }) => {
                if start == end {
                    // Single-line resolution means the anchor landed on a bare
                    // statement, not a multi-line construct — reject.
                    return Err(BlockResolveError::Unresolved(format!(
                        "line {}: `{}` resolved a single-line block (line {}) — \
                         line {} is a bare statement, not the opening line of a \
                         multi-line construct. Use a concrete line range instead.",
                        line_num, op, start, line_num
                    )));
                }

                if op == "insert_after" {
                    // Mirror the parser's `insert after N:` lowering: one
                    // `after_anchor` insert per payload row, anchored on the
                    // block's last line. The `block_start` tag lets the applier's
                    // landing correction slide a body that claims a depth inside
                    // the block back across the block's trailing closer lines.
                    for payload in payloads {
                        resolved.push(Edit::Insert {
                            cursor: Cursor::AfterAnchor(Anchor { line: end }),
                            text: payload.clone(),
                            line_num: *line_num,
                            index: synth_index,
                            mode: None,
                            block_start: Some(start),
                            expected_hash: None,
                        });
                        synth_index += 1;
                    }
                    continue;
                }

                if op == "insert_before" {
                    // Mirror the parser's `insert before N:` lowering: one
                    // `before_anchor` insert per payload row, anchored on the
                    // block's first line.
                    for payload in payloads {
                        resolved.push(Edit::Insert {
                            cursor: Cursor::BeforeAnchor(Anchor { line: start }),
                            text: payload.clone(),
                            line_num: *line_num,
                            index: synth_index,
                            mode: None,
                            block_start: None,
                            expected_hash: None,
                        });
                        synth_index += 1;
                    }
                    continue;
                }

                // Mirror the parser's `replace start.=end:` expansion exactly:
                // one `before_anchor` replacement insert per payload row at
                // `start`, then one delete per line across `[start, end]`.
                // An empty payloads (from `delete_block`) emits no inserts —
                // a pure deletion.
                for payload in payloads {
                    resolved.push(Edit::Insert {
                        cursor: Cursor::BeforeAnchor(Anchor { line: start }),
                        text: payload.clone(),
                        line_num: *line_num,
                        index: synth_index,
                        mode: Some(InsertMode::Replacement),
                        block_start: None,
                        expected_hash: None,
                    });
                    synth_index += 1;
                }
                for line in start..=end {
                    resolved.push(Edit::Delete {
                        anchor: Anchor { line },
                        line_num: *line_num,
                        index: synth_index,
                        expected_hash: None,
                    });
                    synth_index += 1;
                }
            }
        }
    }

    Ok(resolved)
}
