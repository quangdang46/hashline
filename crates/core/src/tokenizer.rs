//! Stateful, line-oriented classifier for hashline diff text.
//!
//! Format shape:
//! ```text
//! [path/to/file.ts#1A2B]
//! replace 5..=7:
//! +literal new line
//! ```

use crate::messages::{ABORT_MARKER, BEGIN_PATCH_MARKER, END_PATCH_MARKER};
use crate::patch_format::{
    HL_CUT_KEYWORD, HL_DELETE_BLOCK_KEYWORD, HL_DELETE_KEYWORD, HL_FILE_HASH_LENGTH,
    HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_INSERT_AFTER, HL_INSERT_AFTER_BLOCK_KEYWORD,
    HL_INSERT_AFTER_BLOCK_SHORT, HL_INSERT_BEFORE, HL_INSERT_BEFORE_BLOCK_KEYWORD, HL_INSERT_HEAD,
    HL_INSERT_KEYWORD, HL_INSERT_TAIL, HL_MAX_RANGE_LINES, HL_MV_KEYWORD, HL_PUT_KEYWORD,
    HL_REM_KEYWORD, HL_REPLACE_BLOCK_KEYWORD, HL_REPLACE_KEYWORD,
};
use crate::types::{Anchor, Cursor, ParsedRange};

const CHAR_CARRIAGE_RETURN: u8 = b'\r';
const CHAR_ZERO: u8 = b'0';
const CHAR_NINE: u8 = b'9';
const CHAR_HASH: u8 = b'#';
const CHAR_TAB: u8 = b'\t';
const CHAR_SPACE: u8 = b' ';
const CHAR_DOT: u8 = b'.';
const CHAR_HYPHEN: u8 = b'-';
#[allow(dead_code)]
const CHAR_EQUALS: u8 = b'=';
const CHAR_UPPER_A: u8 = b'A';
const CHAR_UPPER_F: u8 = b'F';
const CHAR_LOWER_A: u8 = b'a';
const CHAR_LOWER_F: u8 = b'f';
const CHAR_COLON: u8 = b':';
const CHAR_ASTERISK: u8 = b'*';
const CHAR_PAYLOAD_REPLACE: u8 = b'+';
const CHAR_AT: u8 = b'@';
const CHAR_LT: u8 = b'<';

const FILE_PREFIX_LEN: usize = HL_FILE_PREFIX.len();
const FILE_SUFFIX_LEN: usize = HL_FILE_SUFFIX.len();

#[inline]
fn is_digit_code(c: u8) -> bool {
    (CHAR_ZERO..=CHAR_NINE).contains(&c)
}

#[inline]
fn is_hex_digit_code(c: u8) -> bool {
    is_digit_code(c)
        || (CHAR_UPPER_A..=CHAR_UPPER_F).contains(&c)
        || (CHAR_LOWER_A..=CHAR_LOWER_F).contains(&c)
}

#[inline]
fn is_whitespace_code(c: u8) -> bool {
    c == CHAR_SPACE || (CHAR_TAB..=CHAR_CARRIAGE_RETURN).contains(&c)
}

fn skip_whitespace(line: &[u8], mut index: usize, end: usize) -> usize {
    while index < end && is_whitespace_code(line[index]) {
        index += 1;
    }
    index
}

fn trim_end_index(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && is_whitespace_code(bytes[end - 1]) {
        end -= 1;
    }
    end
}

fn is_empty_line(line: &str) -> bool {
    line.is_empty()
}

/// Match an envelope sentinel (`*** Begin Patch`, `*** End Patch`,
/// `*** Abort`). Leading whitespace and trailing text are tolerated —
/// `  *** End Patch — done` is still the sentinel, not payload — but a
/// word boundary is required so `*** End Patchwork` is not.
fn marker_line_equals(line: &str, marker: &str) -> bool {
    let trimmed = line.trim();
    if trimmed == marker {
        return true;
    }
    match trimmed.strip_prefix(marker) {
        Some(rest) => rest.starts_with(char::is_whitespace) || rest.starts_with(':'),
        None => false,
    }
}

pub fn clone_cursor(cursor: &crate::types::Cursor) -> crate::types::Cursor {
    match cursor {
        crate::types::Cursor::BeforeAnchor(a) => {
            crate::types::Cursor::BeforeAnchor(Anchor { line: a.line })
        }
        crate::types::Cursor::AfterAnchor(a) => {
            crate::types::Cursor::AfterAnchor(Anchor { line: a.line })
        }
        crate::types::Cursor::Bof => crate::types::Cursor::Bof,
        crate::types::Cursor::Eof => crate::types::Cursor::Eof,
    }
}

pub struct NumberScan {
    pub line: usize,
    pub next_index: usize,
}

/// Scan a positive line number starting at `index` in `bytes` (slice up to `end`).
pub fn scan_line_number(bytes: &[u8], index: usize, end: usize) -> Option<NumberScan> {
    if index >= end || !is_digit_code(bytes[index]) {
        return None;
    }
    let mut line_number: usize = 0;
    let mut next_index = index;
    while next_index < end {
        let c = bytes[next_index];
        if !is_digit_code(c) {
            break;
        }
        // Saturate instead of overflowing: a wrapped or panicking parse
        // would silently target the wrong line (or abort in debug builds).
        // The resulting huge anchor fails cleanly as "line not found".
        line_number = line_number
            .saturating_mul(10)
            .saturating_add((c - CHAR_ZERO) as usize);
        next_index += 1;
    }
    Some(NumberScan {
        line: line_number,
        next_index,
    })
}

struct RangeScan {
    range: ParsedRange,
    next_index: usize,
    hash: Option<u8>,
}

fn scan_range_separator(bytes: &[u8], index: usize, end: usize) -> Option<usize> {
    let mut cursor = index;
    let mut consumed = false;
    while cursor < end {
        let c = bytes[cursor];
        if is_whitespace_code(c) {
            cursor += 1;
            consumed = true;
            continue;
        }
        if c == CHAR_HYPHEN {
            cursor += 1;
            consumed = true;
            continue;
        }
        if c == CHAR_DOT && cursor + 1 < end && bytes[cursor + 1] == CHAR_DOT {
            cursor += 2;
            // Skip optional trailing '=' (".=" is used as a range separator)
            if cursor < end && bytes[cursor] == CHAR_EQUALS {
                cursor += 1;
            }
            consumed = true;
            continue;
        }
        // Support ".=" as a range separator (oh-my-pi compatible syntax).
        if c == CHAR_DOT && cursor + 1 < end && bytes[cursor + 1] == CHAR_EQUALS {
            cursor += 2;
            consumed = true;
            continue;
        }
        break;
    }
    if !consumed {
        return None;
    }
    if cursor >= end || !is_digit_code(bytes[cursor]) {
        return None;
    }
    Some(cursor)
}

fn scan_header_range(
    bytes: &[u8],
    index: usize,
    end: usize,
    allow_single: bool,
) -> Option<RangeScan> {
    let number_start = skip_whitespace(bytes, index, end);
    let start = scan_line_number(bytes, number_start, end)?;
    let hash_scan = consume_with_hash(bytes, start.next_index, end);
    if hash_scan.malformed {
        return None;
    }
    let after_hash = hash_scan.next;
    let hash = hash_scan.hash;
    let after_first = scan_range_separator(bytes, after_hash, end);
    let after_first = match after_first {
        Some(idx) => idx,
        None => {
            if !allow_single {
                return None;
            }
            return Some(RangeScan {
                range: ParsedRange {
                    start: Anchor { line: start.line },
                    end: Anchor { line: start.line },
                    end_hash: None,
                },
                next_index: skip_whitespace(bytes, after_hash, end),
                hash,
            });
        }
    };
    let end_num = scan_line_number(bytes, after_first, end)?;
    // Reject reversed and absurdly large ranges here so they can never
    // reach the applier (a `DEL 1..999999999` would otherwise try to
    // materialize ~1G anchor edits).
    if end_num.line < start.line {
        return None;
    }
    if end_num.line - start.line >= HL_MAX_RANGE_LINES {
        return None;
    }
    Some(RangeScan {
        range: ParsedRange {
            start: Anchor { line: start.line },
            end: Anchor { line: end_num.line },
            end_hash: None,
        },
        next_index: skip_whitespace(bytes, end_num.next_index, end),
        hash,
    })
}

/// Hunk operation target type.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockTarget {
    Replace(ParsedRange, Option<u8>),
    Block(Anchor, Option<u8>),
    Delete(ParsedRange, Option<u8>),
    DeleteBlock(Anchor, Option<u8>),
    InsertBefore(Anchor, Option<u8>),
    InsertAfter(Anchor, Option<u8>),
    InsertAfterBlock(Anchor, Option<u8>),
    InsertBeforeBlock(Anchor, Option<u8>),
    Cut(ParsedRange, Option<u8>, Option<String>),
    Paste(Cursor, Option<String>),
    Bof,
    Eof,
    Remove,
    MoveTo(String),
}

struct TargetScan {
    target: BlockTarget,
    next_index: usize,
}

fn scan_keyword(bytes: &[u8], index: usize, end: usize, keyword: &str) -> Option<usize> {
    let kw = keyword.as_bytes();
    if index + kw.len() > end {
        return None;
    }
    // Case-insensitive comparison (Bug #89-5)
    #[allow(clippy::manual_ignore_case_cmp)]
    for (i, &k) in kw.iter().enumerate() {
        let b = bytes[index + i];
        if b.to_ascii_lowercase() != k.to_ascii_lowercase() {
            return None;
        }
    }
    let next = index + kw.len();
    if next < end {
        let c = bytes[next];
        if !is_whitespace_code(c) && c != CHAR_COLON && c != CHAR_DOT {
            return None;
        }
    }
    Some(next)
}

fn consume_optional_colon(bytes: &[u8], index: usize, end: usize) -> usize {
    let cursor = skip_whitespace(bytes, index, end);
    if cursor < end && bytes[cursor] == CHAR_COLON {
        skip_whitespace(bytes, cursor + 1, end)
    } else {
        cursor
    }
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Result of scanning an optional `:HH` hash suffix after a line number.
struct HashSuffix {
    /// Index just past the consumed hash suffix (whitespace skipped).
    next: usize,
    /// The parsed 1-byte hash, when a valid suffix was present.
    hash: Option<u8>,
    /// True when a `:` followed the line number but what came after was
    /// not a valid 1-2 hex-digit hash. Callers must reject the operation:
    /// accepting it would silently disable the stale-anchor check.
    malformed: bool,
}

/// Returns the full [`HashSuffix`] for an optional `:HH` hash suffix.
fn consume_with_hash(bytes: &[u8], index: usize, end: usize) -> HashSuffix {
    let cursor = skip_whitespace(bytes, index, end);
    if cursor >= end || bytes[cursor] != CHAR_COLON {
        return HashSuffix {
            next: cursor,
            hash: None,
            malformed: false,
        };
    }
    // After `N:` only whitespace then end-of-line (no hash), or a 1-2
    // hex-digit hash is valid. Anything else (`:zz`, `:abc`, `: +text`)
    // is a malformed hash — report it so the op is rejected instead of
    // silently applying with the stale-anchor check disabled.
    let after = skip_whitespace(bytes, cursor + 1, end);
    if after >= end {
        return HashSuffix {
            next: after,
            hash: None,
            malformed: false,
        };
    }
    if !is_hex_digit_code(bytes[after]) {
        return HashSuffix {
            next: after,
            hash: None,
            malformed: true,
        };
    }
    let mut run_end = after;
    while run_end < end && is_hex_digit_code(bytes[run_end]) {
        run_end += 1;
    }
    // Anchor hashes are exactly 1-2 hex digits; a longer run means the
    // "hash" would be silently truncated (e.g. `SWAP 5:abc` → `ab`).
    if run_end - after > 2 {
        return HashSuffix {
            next: run_end,
            hash: None,
            malformed: true,
        };
    }
    let hash_byte = if run_end - after == 2 {
        (hex_val(bytes[after]) << 4) | hex_val(bytes[after + 1])
    } else {
        hex_val(bytes[after])
    };
    let mut next = run_end;
    if next < end && bytes[next] == CHAR_COLON {
        next += 1;
    }
    HashSuffix {
        next: skip_whitespace(bytes, next, end),
        hash: Some(hash_byte),
        malformed: false,
    }
}

/// Scan an optional `@name` register reference starting at `index`.
///
/// Returns `(Option<name>, Option<next_index>)`:
/// - `(Some(name), Some(next))` — a valid `@name` was consumed.
/// - `(None, None)` — no `@` sigil at `index` (caller decides whether that
///   is allowed, e.g. the anonymous register on `CUT`).
///
/// A `@` sigil followed by a non-empty name is required for `PUT`.
/// Register names are restricted to a conservative charset so sigils like
/// `<`, `@`, and quotes can't be smuggled into a register key (they would
/// corrupt the per-patch clipboard or collide with cursor syntax).
fn is_register_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.'
}

fn scan_register_name(bytes: &[u8], index: usize, end: usize) -> (Option<String>, Option<usize>) {
    let cursor = skip_whitespace(bytes, index, end);
    if cursor >= end || bytes[cursor] != CHAR_AT {
        return (None, None);
    }
    let name_start = cursor + 1;
    if name_start >= end {
        return (None, None);
    }
    let mut name_end = name_start;
    while name_end < end && is_register_char(bytes[name_end]) {
        name_end += 1;
    }
    if name_end == name_start {
        return (None, None);
    }
    let name = std::str::from_utf8(&bytes[name_start..name_end])
        .ok()
        .map(str::to_owned);
    (name, Some(name_end))
}

fn scan_insert_target(bytes: &[u8], index: usize, end: usize) -> Option<TargetScan> {
    // Check for dot-based modifiers: INS.PRE, INS.POST, INS.HEAD, INS.TAIL
    if index < end && bytes[index] == CHAR_DOT {
        let cursor = skip_whitespace(bytes, index + 1, end);

        // INS.PRE N:
        if let Some(before_end) = scan_keyword(bytes, cursor, end, HL_INSERT_BEFORE) {
            let anchor = scan_line_number(bytes, skip_whitespace(bytes, before_end, end), end)?;
            if anchor.line == 0 {
                return None;
            }
            let suffix = consume_with_hash(bytes, anchor.next_index, end);
            if suffix.malformed {
                return None;
            }
            return Some(TargetScan {
                target: BlockTarget::InsertBefore(Anchor { line: anchor.line }, suffix.hash),
                next_index: suffix.next,
            });
        }

        // INS.POST N:
        if let Some(after_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER) {
            let anchor = scan_line_number(bytes, skip_whitespace(bytes, after_end, end), end)?;
            if anchor.line == 0 {
                return None;
            }
            let suffix = consume_with_hash(bytes, anchor.next_index, end);
            if suffix.malformed {
                return None;
            }
            return Some(TargetScan {
                target: BlockTarget::InsertAfter(Anchor { line: anchor.line }, suffix.hash),
                next_index: suffix.next,
            });
        }

        // INS.HEAD:
        if let Some(head_end) = scan_keyword(bytes, cursor, end, HL_INSERT_HEAD) {
            let next = consume_optional_colon(bytes, head_end, end);
            return Some(TargetScan {
                target: BlockTarget::Bof,
                next_index: next,
            });
        }

        // INS.TAIL:
        if let Some(tail_end) = scan_keyword(bytes, cursor, end, HL_INSERT_TAIL) {
            let next = consume_optional_colon(bytes, tail_end, end);
            return Some(TargetScan {
                target: BlockTarget::Eof,
                next_index: next,
            });
        }

        // Has a dot but none of the known modifiers — not a valid INS target
        return None;
    }

    // No dot — treat bare `INS <N>:` as `INS.POST <N>:` (insert after line N).
    // This handles the common case where users write `INS N:` as shorthand.
    let num_start = skip_whitespace(bytes, index, end);
    if num_start >= end || !is_digit_code(bytes[num_start]) {
        return None;
    }
    let anchor = scan_line_number(bytes, num_start, end)?;
    if anchor.line == 0 {
        return None;
    }
    let suffix = consume_with_hash(bytes, anchor.next_index, end);
    if suffix.malformed {
        return None;
    }
    Some(TargetScan {
        target: BlockTarget::InsertAfter(Anchor { line: anchor.line }, suffix.hash),
        next_index: suffix.next,
    })
}

fn scan_mv_dest(line: &str, bytes: &[u8], start: usize, end: usize) -> Option<String> {
    let cursor = skip_whitespace(bytes, start, end);
    if cursor >= end {
        return None;
    }
    if bytes[cursor] == b'\"' || bytes[cursor] == b'\'' {
        let quote = bytes[cursor];
        let mut next = cursor + 1;
        while next < end {
            if bytes[next] == b'\\' && next + 1 < end {
                next += 2;
                continue;
            }
            if bytes[next] == quote {
                let after = skip_whitespace(bytes, next + 1, end);
                if after == end {
                    // Strip quotes
                    let inner = &line[cursor + 1..next];
                    return Some(inner.to_string());
                }
                return None;
            }
            next += 1;
        }
        return None;
    }
    // Unquoted: take remainder trimmed
    let raw = std::str::from_utf8(&bytes[cursor..end])
        .unwrap_or("")
        .trim()
        .to_string();
    if raw.is_empty() { None } else { Some(raw) }
}

fn scan_hunk_anchor(line: &str, bytes: &[u8], start: usize, end: usize) -> Option<TargetScan> {
    let cursor = skip_whitespace(bytes, start, end);

    // SWAP.BLK N:  or SWAP.BLK N:HH M:HH (two-anchor → concrete range)
    if let Some(block_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, block_end, end), end)?;
        if anchor.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, anchor.next_index, end);
        if suffix.malformed {
            return None;
        }
        let hash = suffix.hash;
        let next = suffix.next;
        // Check for two-anchor format: SWAP.BLK N:HH M:HH → treat as concrete Replace range
        if next < end {
            let after_ws = skip_whitespace(bytes, next, end);
            if let Some(second) = scan_line_number(bytes, after_ws, end) {
                if second.line == 0 {
                    return None;
                }
                let second_suffix = consume_with_hash(bytes, second.next_index, end);
                if second_suffix.malformed {
                    return None;
                }
                if skip_whitespace(bytes, second_suffix.next, end) == end {
                    if second.line < anchor.line || second.line - anchor.line >= HL_MAX_RANGE_LINES
                    {
                        return None;
                    }
                    return Some(TargetScan {
                        target: BlockTarget::Replace(
                            ParsedRange {
                                start: Anchor { line: anchor.line },
                                end: Anchor { line: second.line },
                                end_hash: second_suffix.hash,
                            },
                            hash,
                        ),
                        next_index: end,
                    });
                }
            }
        }
        // A `..M` range on a block op is meaningless (the resolver expands
        // the anchor to a syntactic span) — reject it so the lenient
        // trailing-garbage path can't silently drop the `..M` part.
        if next < end && bytes[next] == CHAR_DOT {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::Block(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // SWAP N..=M:
    if let Some(replace_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_KEYWORD) {
        let mut range = scan_header_range(bytes, replace_end, end, true)?;
        if range.range.start.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, range.next_index, end);
        if suffix.malformed {
            return None;
        }
        // A `:HH` after the range end binds the END line (`SWAP 2:d4..4:ee`).
        range.range.end_hash = suffix.hash;
        return Some(TargetScan {
            target: BlockTarget::Replace(range.range, range.hash),
            next_index: suffix.next,
        });
    }

    // DEL.BLK N  or DEL.BLK N:HH M:HH (two-anchor → concrete range)
    if let Some(del_block_end) = scan_keyword(bytes, cursor, end, HL_DELETE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, del_block_end, end), end)?;
        if anchor.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, anchor.next_index, end);
        if suffix.malformed {
            return None;
        }
        let hash = suffix.hash;
        let next = suffix.next;
        // Check for two-anchor format: DEL.BLK N:HH M:HH → treat as concrete Delete range
        if next < end {
            let after_ws = skip_whitespace(bytes, next, end);
            if let Some(second) = scan_line_number(bytes, after_ws, end) {
                if second.line == 0 {
                    return None;
                }
                let second_suffix = consume_with_hash(bytes, second.next_index, end);
                if second_suffix.malformed {
                    return None;
                }
                if skip_whitespace(bytes, second_suffix.next, end) == end {
                    if second.line < anchor.line || second.line - anchor.line >= HL_MAX_RANGE_LINES
                    {
                        return None;
                    }
                    return Some(TargetScan {
                        target: BlockTarget::Delete(
                            ParsedRange {
                                start: Anchor { line: anchor.line },
                                end: Anchor { line: second.line },
                                end_hash: second_suffix.hash,
                            },
                            hash,
                        ),
                        next_index: end,
                    });
                }
            }
            // Two-anchor parse failed; before rejecting, check for colon
            // which blocks fall-through to raw classification
            if next < end && bytes[next] == CHAR_COLON {
                return None;
            }
        }
        // `..M` ranges are not meaningful on block ops — reject rather than
        // silently ignore them (same rule as SWAP.BLK).
        if next < end && bytes[next] == CHAR_DOT {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::DeleteBlock(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // DEL N..=M (no colon, but may have :HH: hash suffix)
    if let Some(delete_end) = scan_keyword(bytes, cursor, end, HL_DELETE_KEYWORD) {
        let mut range = scan_header_range(bytes, delete_end, end, true)?;
        if range.range.start.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, range.next_index, end);
        if suffix.malformed {
            return None;
        }
        range.range.end_hash = suffix.hash;
        let next = suffix.next;
        // Ensure DEL does not take a body (a colon after the optional hash
        // suffix means there's trailing content, which DEL doesn't accept).
        if next < end && bytes[next] == CHAR_COLON {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::Delete(range.range, range.hash),
            next_index: next,
        });
    }

    // INS.BLK.POST N:
    if let Some(iblk_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_end, end), end)?;
        if anchor.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, anchor.next_index, end);
        if suffix.malformed {
            return None;
        }
        if suffix.next < end && bytes[suffix.next] == CHAR_DOT {
            return None; // `..M` ranges are meaningless on block ops
        }
        return Some(TargetScan {
            target: BlockTarget::InsertAfterBlock(Anchor { line: anchor.line }, suffix.hash),
            next_index: suffix.next,
        });
    }

    // INS.BLK.PRE N:  (insert before block — Bug #89-5)
    if let Some(iblk_pre_end) = scan_keyword(bytes, cursor, end, HL_INSERT_BEFORE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_pre_end, end), end)?;
        if anchor.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, anchor.next_index, end);
        if suffix.malformed {
            return None;
        }
        if suffix.next < end && bytes[suffix.next] == CHAR_DOT {
            return None; // `..M` ranges are meaningless on block ops
        }
        return Some(TargetScan {
            target: BlockTarget::InsertBeforeBlock(Anchor { line: anchor.line }, suffix.hash),
            next_index: suffix.next,
        });
    }

    // INS.BLK N:  (bare INS.BLK alias for INS.BLK.POST — Bug #89-5)
    if let Some(iblk_short_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER_BLOCK_SHORT) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_short_end, end), end)?;
        if anchor.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, anchor.next_index, end);
        if suffix.malformed {
            return None;
        }
        if suffix.next < end && bytes[suffix.next] == CHAR_DOT {
            return None; // `..M` ranges are meaningless on block ops
        }
        return Some(TargetScan {
            target: BlockTarget::InsertAfterBlock(Anchor { line: anchor.line }, suffix.hash),
            next_index: suffix.next,
        });
    }

    // INS.xxx
    if let Some(insert_end) = scan_keyword(bytes, cursor, end, HL_INSERT_KEYWORD) {
        return scan_insert_target(bytes, insert_end, end);
    }

    // CUT N..=M @name — capture original lines into a register and delete.
    if let Some(cut_end) = scan_keyword(bytes, cursor, end, HL_CUT_KEYWORD) {
        let mut range = scan_header_range(bytes, cut_end, end, true)?;
        if range.range.start.line == 0 {
            return None;
        }
        let suffix = consume_with_hash(bytes, range.next_index, end);
        if suffix.malformed {
            return None;
        }
        range.range.end_hash = suffix.hash;
        let next = suffix.next;
        let register = scan_register_name(bytes, next, end);
        // Named register: `CUT 5..9 @fn` — the name must consume to end.
        if let Some(reg_end) = register.1 {
            if skip_whitespace(bytes, reg_end, end) == end {
                return Some(TargetScan {
                    target: BlockTarget::Cut(range.range, range.hash, register.0),
                    next_index: end,
                });
            }
            return None;
        }
        // No `@` sigil — anonymous register. Everything must be consumed.
        if next == end {
            return Some(TargetScan {
                target: BlockTarget::Cut(range.range, range.hash, None),
                next_index: end,
            });
        }
        return None;
    }

    // PUT [@name] <N: — insert a captured register before line N.
    if let Some(put_end) = scan_keyword(bytes, cursor, end, HL_PUT_KEYWORD) {
        let register = scan_register_name(bytes, put_end, end);
        // `@name` is optional: a bare `PUT <N` pastes the anonymous register.
        // Cursor: `<N:` inserts before line N; bare `PUT [@name]` defaults to
        // file head (before line 1).
        let after = match register.1 {
            Some(reg_end) => skip_whitespace(bytes, reg_end, end),
            None => skip_whitespace(bytes, put_end, end),
        };
        let cursor_target = if after < end && bytes[after] == CHAR_LT {
            let num_start = skip_whitespace(bytes, after + 1, end);
            let anchor = scan_line_number(bytes, num_start, end)?;
            if anchor.line == 0 {
                return None;
            }
            let suffix = consume_with_hash(bytes, anchor.next_index, end);
            if suffix.malformed || suffix.next != end {
                return None;
            }
            Cursor::BeforeAnchor(Anchor { line: anchor.line })
        } else if after == end {
            Cursor::Bof
        } else {
            return None;
        };
        return Some(TargetScan {
            target: BlockTarget::Paste(cursor_target, register.0),
            next_index: end,
        });
    }

    // REM — delete whole file (no payload)
    if let Some(rem_end) = scan_keyword(bytes, cursor, end, HL_REM_KEYWORD) {
        let next = consume_optional_colon(bytes, rem_end, end);
        if next == end {
            return Some(TargetScan {
                target: BlockTarget::Remove,
                next_index: next,
            });
        }
    }

    // MV path — rename/move file to destination path
    if let Some(mv_end) = scan_keyword(bytes, cursor, end, HL_MV_KEYWORD) {
        if let Some(dest) = scan_mv_dest(line, bytes, mv_end, end) {
            if !dest.is_empty() {
                return Some(TargetScan {
                    target: BlockTarget::MoveTo(dest),
                    next_index: end,
                });
            }
        }
    }

    None
}

pub(crate) fn try_parse_hunk_header(line: &str) -> Option<BlockTarget> {
    let end = trim_end_index(line);
    let bytes = line.as_bytes();
    let start = skip_whitespace(bytes, 0, end);
    if start >= end {
        return None;
    }
    let scan = scan_hunk_anchor(line, bytes, start, end)?;
    if scan.next_index != end {
        // Fix #113/#115: Be lenient about trailing garbage after a valid scan.
        // Agents may send malformed hashes like `SWAP 99:zz` where `zz` is not
        // valid hex. The scan succeeded (recognized the operation keyword and
        // line number), but trailing non-whitespace chars remain. Accept the
        // operation — the applier will handle the stale/out-of-range anchor.
        // Only reject if trailing chars are whitespace (indicates a valid range
        // continuation that failed to parse, e.g. `SWAP 2 `).
        let trailing = &bytes[scan.next_index..end];
        if trailing.iter().all(|&b| is_whitespace_code(b)) {
            return None;
        }
        // Trailing non-whitespace after a valid scan: accept with leniency.
    }
    Some(scan.target)
}

/// Strip common apply-patch path noise that LLMs prepend to file headers.
///
/// Strips:
/// - 1-3 leading `*` characters (4+ stars are preserved since they could be envelope markers)
/// - Case-insensitive `(Update|Add|Delete|Move)[^A-Za-z0-9]*(File|to)?[^A-Za-z0-9]*:`
///
/// Returns the cleaned path string.
pub fn strip_apply_patch_path_noise(path_text: &str) -> String {
    if path_text.is_empty() {
        return String::new();
    }

    // Step 1: Strip 1-3 leading '*' characters (but not 4+ — those are envelope markers)
    let s = {
        let bytes = path_text.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len && i < 3 && bytes[i] == CHAR_ASTERISK {
            i += 1;
        }
        if i > 0 { &path_text[i..] } else { path_text }
    };
    let s = s.trim_start();

    if s.is_empty() {
        return s.to_string();
    }

    // Step 2: Strip case-insensitive keyword prefix.
    // Pattern: (Update|Add|Delete|Move)[^A-Za-z0-9]*(File|to)?[^A-Za-z0-9]*:
    let lower = s.to_lowercase();
    let mut result: Option<String> = None;

    // Try each keyword prefix in order; take the longest match wins.
    for &(keyword, kw_len) in &[("update", 6usize), ("delete", 6), ("add", 3), ("move", 4)] {
        if !lower.starts_with(keyword) {
            continue;
        }
        let after_kw = &s[kw_len..];

        // Skip non-alphanumeric separator characters
        let after_sep =
            after_kw.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':');

        // Check for optional "File" or "to"
        let after_opt = {
            let after_sep_lower = after_sep.to_lowercase();
            if after_sep_lower.starts_with("file") {
                let after_file = &after_sep[4..];
                after_file.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':')
            } else if after_sep_lower.starts_with("to") {
                let after_to = &after_sep[2..];
                after_to.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':')
            } else {
                after_sep
            }
        };

        // Expect ':'
        if let Some(rest) = after_opt.strip_prefix(':') {
            let cleaned = rest.trim_start().to_string();
            result = Some(cleaned);
            break;
        }
    }

    result.unwrap_or_else(|| s.to_string())
}

fn try_parse_header(line: &str) -> Option<HeaderResult> {
    if !line.starts_with(HL_FILE_PREFIX) {
        return None;
    }
    let end = trim_end_index(line);
    let bytes = line.as_bytes();
    if FILE_PREFIX_LEN + FILE_SUFFIX_LEN >= end {
        return None;
    }
    if !line.ends_with(HL_FILE_SUFFIX) || end < FILE_SUFFIX_LEN {
        return None;
    }
    let body_end = end - FILE_SUFFIX_LEN;
    if FILE_PREFIX_LEN >= body_end {
        return None;
    }

    // Detect trailing #XXXX tag
    let mut path_end = body_end;
    let mut file_hash: Option<String> = None;
    let tag_start = body_end.saturating_sub(HL_FILE_HASH_LENGTH + 1);
    if tag_start >= FILE_PREFIX_LEN && bytes[tag_start] == CHAR_HASH {
        let mut all_hex = true;
        for probe in (tag_start + 1)..body_end {
            if !is_hex_digit_code(bytes[probe]) {
                all_hex = false;
                break;
            }
        }
        if all_hex {
            path_end = tag_start;
            let hash_slice = &line[tag_start + 1..body_end];
            file_hash = Some(hash_slice.to_uppercase());
        }
    }

    // No '#' allowed in the path portion
    for i in FILE_PREFIX_LEN..path_end {
        if bytes[i] == CHAR_HASH {
            return None;
        }
    }

    if path_end == FILE_PREFIX_LEN {
        return None;
    }
    let path = &line[FILE_PREFIX_LEN..path_end];
    let path = strip_apply_patch_path_noise(path);
    if path.is_empty() {
        return None;
    }
    Some(HeaderResult { path, file_hash })
}

struct HeaderResult {
    path: String,
    file_hash: Option<String>,
}

/// Attempt to recover a header from a bracketed line that the strict header parser
/// rejected (e.g., `[*** Update File:foo.ts#CB5A]`).
///
/// This function:
/// - Checks that the line starts with `[` and ends with `]`
/// - Strips noise from the inner body via `strip_apply_patch_path_noise`
/// - Looks for `#XXXX` hash suffix
/// - Returns path + optional hash
fn try_parse_recovery_header(line: &str) -> Option<HeaderResult> {
    let line = line.trim();
    if !line.starts_with(HL_FILE_PREFIX) || !line.ends_with(HL_FILE_SUFFIX) {
        return None;
    }

    let end = trim_end_index(line);
    let body_end = end - FILE_SUFFIX_LEN;

    if FILE_PREFIX_LEN >= body_end {
        return None;
    }

    let raw_body = &line[FILE_PREFIX_LEN..body_end];

    // Detect trailing #XXXX tag in the raw body
    let mut path_end = raw_body.len();
    let mut file_hash: Option<String> = None;
    let tag_start = raw_body.len().saturating_sub(HL_FILE_HASH_LENGTH + 1);
    let raw_bytes = raw_body.as_bytes();
    if tag_start > 0 && raw_bytes[tag_start] == CHAR_HASH {
        let mut all_hex = true;
        for probe in (tag_start + 1)..raw_body.len() {
            if !is_hex_digit_code(raw_bytes[probe]) {
                all_hex = false;
                break;
            }
        }
        if all_hex {
            path_end = tag_start;
            let hash_slice = &raw_body[tag_start + 1..];
            file_hash = Some(hash_slice.to_uppercase());
        }
    }

    let path_section = &raw_body[..path_end];
    let path = strip_apply_patch_path_noise(path_section);
    if path.is_empty() {
        return None;
    }

    // No '#' allowed in the cleaned path portion
    if path.contains('#') {
        return None;
    }

    Some(HeaderResult { path, file_hash })
}

/// A single classified line from the tokenizer.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Blank {
        line_num: usize,
    },
    EnvelopeBegin {
        line_num: usize,
    },
    EnvelopeEnd {
        line_num: usize,
    },
    Abort {
        line_num: usize,
    },
    Header {
        line_num: usize,
        path: String,
        file_hash: Option<String>,
    },
    OpBlock {
        line_num: usize,
        target: BlockTarget,
    },
    PayloadLiteral {
        line_num: usize,
        text: String,
    },
    Raw {
        line_num: usize,
        text: String,
    },
}

fn classify_line(line: &str, line_num: usize) -> Token {
    if is_empty_line(line) {
        return Token::Blank { line_num };
    }
    if marker_line_equals(line, BEGIN_PATCH_MARKER) {
        return Token::EnvelopeBegin { line_num };
    }
    if marker_line_equals(line, END_PATCH_MARKER) {
        return Token::EnvelopeEnd { line_num };
    }
    if marker_line_equals(line, ABORT_MARKER) {
        return Token::Abort { line_num };
    }

    let first_byte = line.as_bytes().first().copied().unwrap_or(0);

    if line.starts_with(HL_FILE_PREFIX) {
        if let Some(hr) = try_parse_header(line) {
            return Token::Header {
                line_num,
                path: hr.path,
                file_hash: hr.file_hash,
            };
        }
        // Fallback: try recovery header for noisy for
        // [*** Update File:foo.ts#CB5A]
        if let Some(hr) = try_parse_recovery_header(line) {
            return Token::Header {
                line_num,
                path: hr.path,
                file_hash: hr.file_hash,
            };
        }
    }

    let lead = skip_whitespace(line.as_bytes(), 0, line.len());
    let trimmed = &line[lead..];
    let lower = trimmed.to_ascii_lowercase();
    let is_hunk_lead = lower
        .as_bytes()
        .starts_with(HL_REPLACE_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_DELETE_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_INSERT_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_REM_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_MV_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_CUT_KEYWORD.to_ascii_lowercase().as_bytes())
        || lower
            .as_bytes()
            .starts_with(HL_PUT_KEYWORD.to_ascii_lowercase().as_bytes());
    if is_hunk_lead {
        if let Some(target) = try_parse_hunk_header(line) {
            return Token::OpBlock { line_num, target };
        }
    }

    if first_byte == CHAR_PAYLOAD_REPLACE {
        return Token::PayloadLiteral {
            line_num,
            text: line[1..].to_owned(),
        };
    }

    Token::Raw {
        line_num,
        text: line.to_owned(),
    }
}

/// Classifies a single hashline line.
pub struct Tokenizer;

impl Tokenizer {
    pub fn tokenize(&self, line: &str, line_num: usize) -> Token {
        classify_line(line, line_num)
    }

    pub fn is_header(&self, line: &str) -> bool {
        try_parse_header(line).is_some() || try_parse_recovery_header(line).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── strip_apply_patch_path_noise ──────────────────────────────────

    #[test]
    fn test_strip_noise_no_noise() {
        // Clean path passes through unchanged
        assert_eq!(strip_apply_patch_path_noise("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_strip_noise_empty() {
        assert_eq!(strip_apply_patch_path_noise(""), "");
    }

    #[test]
    fn test_strip_noise_leading_stars() {
        assert_eq!(strip_apply_patch_path_noise("*foo.ts"), "foo.ts");
        assert_eq!(strip_apply_patch_path_noise("**foo.ts"), "foo.ts");
        assert_eq!(strip_apply_patch_path_noise("***foo.ts"), "foo.ts");
        // 4+ stars: only the first 3 are stripped
        assert_eq!(strip_apply_patch_path_noise("****foo.ts"), "*foo.ts");
    }

    #[test]
    fn test_strip_noise_star_update_file() {
        assert_eq!(
            strip_apply_patch_path_noise("*** Update File:foo.ts#CB5A"),
            "foo.ts#CB5A"
        );
    }

    #[test]
    fn test_strip_noise_update_file() {
        assert_eq!(strip_apply_patch_path_noise("Update File:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_update_file_case() {
        assert_eq!(strip_apply_patch_path_noise("update file:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_add_file() {
        assert_eq!(strip_apply_patch_path_noise("Add File:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_delete_file() {
        assert_eq!(strip_apply_patch_path_noise("Delete File:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_move_to() {
        assert_eq!(strip_apply_patch_path_noise("Move to:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_update_no_file() {
        assert_eq!(strip_apply_patch_path_noise("Update:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_add_no_separator() {
        assert_eq!(strip_apply_patch_path_noise("Add:foo.ts"), "foo.ts");
    }

    #[test]
    fn test_strip_noise_leading_stars_with_whitespace() {
        assert_eq!(
            strip_apply_patch_path_noise("*** Update File:  foo.ts"),
            "foo.ts"
        );
    }

    #[test]
    fn test_strip_noise_star_then_update() {
        assert_eq!(
            strip_apply_patch_path_noise("**Update File:foo.ts"),
            "foo.ts"
        );
    }

    #[test]
    fn test_strip_noise_invalid_prefix_not_stripped() {
        // Unknown prefix should remain
        assert_eq!(
            strip_apply_patch_path_noise("Modify:foo.ts"),
            "Modify:foo.ts"
        );
    }

    #[test]
    fn test_strip_noise_path_with_hash() {
        // Path with hash should keep hash
        assert_eq!(
            strip_apply_patch_path_noise("***Update File:src/lib.ts#1A2B"),
            "src/lib.ts#1A2B"
        );
    }

    // ─── try_parse_recovery_header ─────────────────────────────────────

    #[test]
    fn test_recovery_noisy_bracket() {
        // [*** Update File:foo.ts#CB5A]
        let hr = try_parse_recovery_header("[*** Update File:foo.ts#CB5A]").unwrap();
        assert_eq!(hr.path, "foo.ts");
        assert_eq!(hr.file_hash.as_deref(), Some("CB5A"));
    }

    #[test]
    fn test_recovery_noisy_no_hash() {
        let hr = try_parse_recovery_header("[** Add File:bar.rs]").unwrap();
        assert_eq!(hr.path, "bar.rs");
        assert_eq!(hr.file_hash, None);
    }

    #[test]
    fn test_recovery_normal_header_returns_none() {
        // Normal headers are handled by try_parse_header, not recovery
        let hr = try_parse_recovery_header("[foo.ts#A1B2]").unwrap();
        assert_eq!(hr.path, "foo.ts");
        assert_eq!(hr.file_hash.as_deref(), Some("A1B2"));
    }

    #[test]
    fn test_recovery_unclosed_bracket() {
        assert!(try_parse_recovery_header("[*** Update File:foo.ts#CB5A").is_none());
    }

    #[test]
    fn test_recovery_empty_body() {
        assert!(try_parse_recovery_header("[]").is_none());
    }

    // ─── Integration: classify_line ────────────────────────────────────

    fn check_token_path(token: &Token, expected_path: &str) {
        match token {
            Token::Header { path, .. } => assert_eq!(path, expected_path),
            _ => panic!("expected Header token, got {:?}", token),
        }
    }

    #[test]
    fn test_classify_noisy_header() {
        let t = classify_line("[*** Update File:src/lib.ts#1A2B]", 1);
        check_token_path(&t, "src/lib.ts");
    }

    #[test]
    fn test_classify_noisy_header_no_hash() {
        let t = classify_line("[*Add File:new.rs]", 1);
        check_token_path(&t, "new.rs");
    }

    #[test]
    fn test_classify_clean_header_still_works() {
        let t = classify_line("[src/lib.ts#1A2B]", 1);
        check_token_path(&t, "src/lib.ts");
    }

    #[test]
    fn test_is_header_noisy() {
        let tokenizer = Tokenizer;
        assert!(tokenizer.is_header("[*** Update File:foo.ts#CB5A]"));
        assert!(tokenizer.is_header("[**Add File:bar.rs]"));
        assert!(tokenizer.is_header("[Delete File:baz.rs]"));
        // Clean headers still work
        assert!(tokenizer.is_header("[clean.ts#1A2B]"));
    }

    // ---- P9: CUT / PUT tokenization ----

    fn classify_op(line: &str) -> BlockTarget {
        match classify_line(line, 1) {
            Token::OpBlock { target, .. } => target,
            other => panic!("expected OpBlock token, got {other:?}"),
        }
    }

    #[test]
    fn test_cut_named_register() {
        match classify_op("CUT 5..9 @fn") {
            BlockTarget::Cut(range, hash, register) => {
                assert_eq!(range.start.line, 5);
                assert_eq!(range.end.line, 9);
                assert_eq!(hash, None);
                assert_eq!(register.as_deref(), Some("fn"));
            }
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn test_cut_anonymous_register() {
        match classify_op("CUT 5..9") {
            BlockTarget::Cut(range, _, register) => {
                assert_eq!(range.start.line, 5);
                assert_eq!(range.end.line, 9);
                assert_eq!(register, None);
            }
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn test_cut_single_line_with_hash() {
        match classify_op("CUT 2:ab @fn") {
            BlockTarget::Cut(range, hash, register) => {
                assert_eq!(range.start.line, 2);
                assert_eq!(range.end.line, 2);
                assert_eq!(hash, Some(0xab));
                assert_eq!(register.as_deref(), Some("fn"));
            }
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn test_put_before_anchor_named() {
        match classify_op("PUT @fn <20") {
            BlockTarget::Paste(Cursor::BeforeAnchor(a), register) => {
                assert_eq!(a.line, 20);
                assert_eq!(register.as_deref(), Some("fn"));
            }
            other => panic!("expected Paste before-anchor, got {other:?}"),
        }
    }

    #[test]
    fn test_put_bare_defaults_to_bof() {
        match classify_op("PUT @fn") {
            BlockTarget::Paste(Cursor::Bof, register) => {
                assert_eq!(register.as_deref(), Some("fn"));
            }
            other => panic!("expected Paste Bof, got {other:?}"),
        }
    }

    #[test]
    fn test_put_anonymous_before_anchor() {
        match classify_op("PUT <4") {
            BlockTarget::Paste(Cursor::BeforeAnchor(a), register) => {
                assert_eq!(a.line, 4);
                assert_eq!(register, None);
            }
            other => panic!("expected Paste before-anchor, got {other:?}"),
        }
    }

    #[test]
    fn test_put_malformed_rejected() {
        // PUT with a trailing colon is not a valid register paste.
        let t = classify_line("PUT @fn:", 1);
        assert!(!matches!(t, Token::OpBlock { .. }));
    }

    #[test]
    fn test_put_after_anchor_not_supported() {
        // v1 supports only the before-line cursor; `PUT @fn >N` is rejected.
        let t = classify_line("PUT @fn >5", 1);
        assert!(!matches!(t, Token::OpBlock { .. }));
    }

    // ---- Audit hardening: overflow, malformed hashes, ranges, registers ----

    #[test]
    fn huge_line_number_saturates_no_panic() {
        // `SWAP 99999999999999999999999999:` used to panic in debug and wrap
        // in release (editing the wrong line). It must parse to a saturated
        // anchor that fails cleanly at apply time.
        match classify_op("SWAP 99999999999999999999999999:") {
            BlockTarget::Replace(range, _) => {
                assert_eq!(range.start.line, usize::MAX);
                assert_eq!(range.end.line, usize::MAX);
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn non_hex_hash_rejected() {
        // `SWAP 2:zz:` silently disabled the stale-anchor check before.
        let t = classify_line("SWAP 2:zz:", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
    }

    #[test]
    fn overlong_hash_rejected() {
        // `SWAP 5:abc` used to truncate the hash to `ab`.
        let t = classify_line("SWAP 5:abc", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
        let t = classify_line("SWAP 5:abcd:", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
    }

    #[test]
    fn valid_hashes_still_parse() {
        match classify_op("SWAP 2:67:") {
            BlockTarget::Replace(range, hash) => {
                assert_eq!(range.start.line, 2);
                assert_eq!(hash, Some(0x67));
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        match classify_op("SWAP 2:6:") {
            BlockTarget::Replace(_, hash) => assert_eq!(hash, Some(0x06)),
            other => panic!("expected Replace, got {other:?}"),
        }
        match classify_op("SWAP 2:") {
            BlockTarget::Replace(_, hash) => assert_eq!(hash, None),
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn range_end_hash_is_captured() {
        // `DEL 2:d4..4:ee` — the end hash used to be parsed then discarded.
        match classify_op("DEL 2:d4..4:ee") {
            BlockTarget::Delete(range, hash) => {
                assert_eq!(range.start.line, 2);
                assert_eq!(range.end.line, 4);
                assert_eq!(hash, Some(0xd4));
                assert_eq!(range.end_hash, Some(0xee));
            }
            other => panic!("expected Delete, got {other:?}"),
        }
        // Trailing hash on a range binds the END line.
        match classify_op("SWAP 2..3:67:") {
            BlockTarget::Replace(range, hash) => {
                assert_eq!(hash, None);
                assert_eq!(range.end_hash, Some(0x67));
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn reversed_range_rejected() {
        let t = classify_line("SWAP 5..0:", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
        let t = classify_line("DEL 9..5", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
    }

    #[test]
    fn oversized_range_rejected() {
        let t = classify_line("DEL 1..999999999", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
        let t = classify_line("SWAP 1..2000000:", 1);
        assert!(!matches!(t, Token::OpBlock { .. }), "got {t:?}");
    }

    #[test]
    fn register_charset_restricted() {
        for bad in ["CUT 5 @a<b", "CUT 5 @a@b", "CUT 5 @'fn'", "PUT @a<b <2"] {
            let t = classify_line(bad, 1);
            assert!(!matches!(t, Token::OpBlock { .. }), "{bad} got {t:?}");
        }
        // Legit names still work, including - _ . inside the name.
        match classify_op("CUT 5 @my-reg_1.2") {
            BlockTarget::Cut(_, _, reg) => assert_eq!(reg.as_deref(), Some("my-reg_1.2")),
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn envelope_markers_tolerate_whitespace_and_trailing_text() {
        assert!(matches!(
            classify_line("  *** End Patch — done", 1),
            Token::EnvelopeEnd { .. }
        ));
        assert!(matches!(
            classify_line("*** Abort: user cancelled", 1),
            Token::Abort { .. }
        ));
        assert!(matches!(
            classify_line("\t*** Begin Patch extra", 1),
            Token::EnvelopeBegin { .. }
        ));
        // No word boundary → not a sentinel.
        let t = classify_line("*** End Patchwork", 1);
        assert!(!matches!(t, Token::EnvelopeEnd { .. }));
    }
}
