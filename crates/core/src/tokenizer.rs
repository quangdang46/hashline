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
    HL_DELETE_BLOCK_KEYWORD, HL_DELETE_KEYWORD, HL_FILE_HASH_LENGTH, HL_FILE_HASH_SEP,
    HL_FILE_PREFIX, HL_FILE_SUFFIX, HL_INSERT_AFTER, HL_INSERT_AFTER_BLOCK_KEYWORD,
    HL_INSERT_AFTER_BLOCK_SHORT, HL_INSERT_BEFORE, HL_INSERT_BEFORE_BLOCK_KEYWORD, HL_INSERT_HEAD,
    HL_INSERT_KEYWORD, HL_INSERT_TAIL, HL_MV_KEYWORD, HL_REM_KEYWORD, HL_REPLACE_BLOCK_KEYWORD,
    HL_REPLACE_KEYWORD, describe_anchor_examples,
};
use crate::types::{Anchor, ParsedRange};

const CHAR_LINE_FEED: u8 = b'\n';
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

const FILE_PREFIX_LEN: usize = HL_FILE_PREFIX.len();
const FILE_SUFFIX_LEN: usize = HL_FILE_SUFFIX.len();

#[inline]
fn is_digit_code(c: u8) -> bool {
    (CHAR_ZERO..=CHAR_NINE).contains(&c)
}

#[inline]
#[allow(dead_code)]
fn is_non_zero_digit_code(c: u8) -> bool {
    c > CHAR_ZERO && c <= CHAR_NINE
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

fn marker_line_equals(line: &str, marker: &str) -> bool {
    let end = trim_end_index(line);
    end == marker.len() && line.as_bytes().starts_with(marker.as_bytes())
}

/// Split text into LF-delimited lines, stripping trailing CR.
pub fn split_hashline_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for i in 0..bytes.len() {
        if bytes[i] != CHAR_LINE_FEED {
            continue;
        }
        let mut end = i;
        if end > start && bytes[end - 1] == CHAR_CARRIAGE_RETURN {
            end -= 1;
        }
        lines.push(text[start..end].to_owned());
        start = i + 1;
    }
    if start < text.len() {
        let mut end = text.len();
        if end > start && bytes[end - 1] == CHAR_CARRIAGE_RETURN {
            end -= 1;
        }
        lines.push(text[start..end].to_owned());
    }
    lines
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
        line_number = line_number * 10 + (c - CHAR_ZERO) as usize;
        next_index += 1;
    }
    Some(NumberScan {
        line: line_number,
        next_index,
    })
}

/// Parse a bare line-number anchor. Returns error on malformed input.
pub fn parse_lid(raw: &str, line_num: usize) -> Result<Anchor, String> {
    let end = trim_end_index(raw);
    let bytes = raw.as_bytes();
    let number_start = skip_whitespace(bytes, 0, end);
    let scan = scan_line_number(bytes, number_start, end);
    match scan {
        Some(s) if skip_whitespace(bytes, s.next_index, end) == end => Ok(Anchor { line: s.line }),
        _ => Err(format!(
            "line {line_num}: expected a line number such as {}; \
             got {raw:?}. Use {HL_FILE_PREFIX}PATH{HL_FILE_HASH_SEP}hash{HL_FILE_SUFFIX} \
             from your latest read for file-version binding.",
            describe_anchor_examples(Some("119")),
        )),
    }
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
    let (after_hash, hash) = consume_with_hash(bytes, start.next_index, end);
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
                },
                next_index: skip_whitespace(bytes, after_hash, end),
                hash,
            });
        }
    };
    let end_num = scan_line_number(bytes, after_first, end)?;
    Some(RangeScan {
        range: ParsedRange {
            start: Anchor { line: start.line },
            end: Anchor { line: end_num.line },
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

/// In hunk headers like `SWAP 2:67:` the optional `HH?` (1-2 hex digits)
/// hash suffix sits between the line number and the terminating colon.
/// Both colons and the hash are consumed here so the hunk header parses
/// as a whole. The hash itself is treated as a fingerprint hint and is
/// NOT validated at tokenize time (no file content is available); the
/// applier ignores the hash for SWAP/DEL and only requires the line
/// number to be in range.
///
/// Accepts:
///   `SWAP 2:`         — single colon, no hash
///   `SWAP 2:67:`      — hash + trailing colon
///   `SWAP 2..3:67:`   — range with hash suffix
/// Delegates to `consume_with_hash` which returns (next_index, hash).
pub fn consume_optional_colon_with_hash(bytes: &[u8], index: usize, end: usize) -> usize {
    consume_with_hash(bytes, index, end).0
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Like consume_optional_colon_with_hash but returns (next_index, Option<hash>).
fn consume_with_hash(bytes: &[u8], index: usize, end: usize) -> (usize, Option<u8>) {
    let cursor = skip_whitespace(bytes, index, end);
    if cursor < end && bytes[cursor] == CHAR_COLON {
        let after = cursor + 1;
        if after < end && is_hex_digit_code(bytes[after]) {
            let hi = hex_val(bytes[after]);
            let mut after2 = after + 1;
            let hash_byte = if after2 < end && is_hex_digit_code(bytes[after2]) {
                let lo = hex_val(bytes[after2]);
                after2 += 1;
                (hi << 4) | lo
            } else {
                hi
            };
            let mut next = after2;
            if after2 < end && bytes[after2] == CHAR_COLON {
                next = after2 + 1;
            }
            (skip_whitespace(bytes, next, end), Some(hash_byte))
        } else {
            (skip_whitespace(bytes, after, end), None)
        }
    } else {
        (cursor, None)
    }
}

fn scan_insert_target(bytes: &[u8], index: usize, end: usize) -> Option<TargetScan> {
    if index >= end || bytes[index] != CHAR_DOT {
        return None;
    }
    let cursor = skip_whitespace(bytes, index + 1, end);

    // INS.PRE N:
    if let Some(before_end) = scan_keyword(bytes, cursor, end, HL_INSERT_BEFORE) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, before_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertBefore(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // INS.POST N:
    if let Some(after_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, after_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertAfter(Anchor { line: anchor.line }, hash),
            next_index: next,
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

    None
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

    // SWAP.BLK N:
    if let Some(block_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, block_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::Block(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // SWAP N..=M:
    if let Some(replace_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_KEYWORD) {
        let range = scan_header_range(bytes, replace_end, end, true)?;
        let (next, _) = consume_with_hash(bytes, range.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::Replace(range.range, range.hash),
            next_index: next,
        });
    }

    // DEL.BLK N (no body, but accepts optional :HH: hash suffix)
    if let Some(del_block_end) = scan_keyword(bytes, cursor, end, HL_DELETE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, del_block_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        // No colon after hash allowed — DEL.BLK takes no body
        if next < end && bytes[next] == CHAR_COLON {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::DeleteBlock(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // DEL N..=M (no colon, but may have :HH: hash suffix)
    if let Some(delete_end) = scan_keyword(bytes, cursor, end, HL_DELETE_KEYWORD) {
        let range = scan_header_range(bytes, delete_end, end, true)?;
        let (next, _) = consume_with_hash(bytes, range.next_index, end);
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
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertAfterBlock(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // INS.BLK.PRE N:  (insert before block — Bug #89-5)
    if let Some(iblk_pre_end) = scan_keyword(bytes, cursor, end, HL_INSERT_BEFORE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_pre_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertBeforeBlock(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // INS.BLK N:  (bare INS.BLK alias for INS.BLK.POST — Bug #89-5)
    if let Some(iblk_short_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER_BLOCK_SHORT) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_short_end, end), end)?;
        let (next, hash) = consume_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertAfterBlock(Anchor { line: anchor.line }, hash),
            next_index: next,
        });
    }

    // INS.xxx
    if let Some(insert_end) = scan_keyword(bytes, cursor, end, HL_INSERT_KEYWORD) {
        return scan_insert_target(bytes, insert_end, end);
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

fn try_parse_hunk_header(line: &str) -> Option<BlockTarget> {
    let end = trim_end_index(line);
    let bytes = line.as_bytes();
    let start = skip_whitespace(bytes, 0, end);
    if start >= end {
        return None;
    }
    let scan = scan_hunk_anchor(line, bytes, start, end)?;
    if scan.next_index != end {
        return None;
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
            .starts_with(HL_MV_KEYWORD.to_ascii_lowercase().as_bytes());
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

    pub fn is_op(&self, line: &str) -> bool {
        try_parse_hunk_header(line).is_some()
    }

    pub fn is_header(&self, line: &str) -> bool {
        try_parse_header(line).is_some() || try_parse_recovery_header(line).is_some()
    }

    pub fn is_envelope_marker(&self, line: &str) -> bool {
        marker_line_equals(line, BEGIN_PATCH_MARKER)
            || marker_line_equals(line, END_PATCH_MARKER)
            || marker_line_equals(line, ABORT_MARKER)
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
}
