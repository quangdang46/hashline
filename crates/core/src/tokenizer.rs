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
    HL_INSERT_BEFORE, HL_INSERT_HEAD, HL_INSERT_KEYWORD, HL_INSERT_TAIL, HL_REPLACE_BLOCK_KEYWORD,
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
const CHAR_PAYLOAD_REPLACE: u8 = b'+';

const FILE_PREFIX_LEN: usize = HL_FILE_PREFIX.len();
const FILE_SUFFIX_LEN: usize = HL_FILE_SUFFIX.len();

#[inline]
fn is_digit_code(c: u8) -> bool {
    (CHAR_ZERO..=CHAR_NINE).contains(&c)
}

#[inline]
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
    if index >= end || !is_non_zero_digit_code(bytes[index]) {
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
    if cursor >= end || !is_non_zero_digit_code(bytes[cursor]) {
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
    let after_first = scan_range_separator(bytes, start.next_index, end);
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
                next_index: skip_whitespace(bytes, start.next_index, end),
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
    })
}

/// Hunk operation target type.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockTarget {
    Replace(ParsedRange),
    Block(Anchor),
    Delete(ParsedRange),
    DeleteBlock(Anchor),
    InsertBefore(Anchor),
    InsertAfter(Anchor),
    InsertAfterBlock(Anchor),
    Bof,
    Eof,
}

struct TargetScan {
    target: BlockTarget,
    next_index: usize,
}

fn scan_keyword(bytes: &[u8], index: usize, end: usize, keyword: &str) -> Option<usize> {
    let kw = keyword.as_bytes();
    if index + kw.len() > end || !bytes[index..].starts_with(kw) {
        return None;
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
fn consume_optional_colon_with_hash(bytes: &[u8], index: usize, end: usize) -> usize {
    let cursor = skip_whitespace(bytes, index, end);
    if cursor < end && bytes[cursor] == CHAR_COLON {
        let mut after = cursor + 1;
        // Try to consume 1-2 hex digits. If we find any, also consume the
        // trailing terminator colon. If no hex digits are present, this
        // colon IS the terminator and was already consumed.
        if after < end && is_hex_digit_code(bytes[after]) {
            after += 1;
            if after < end && is_hex_digit_code(bytes[after]) {
                after += 1;
            }
            // The hash's terminator colon (e.g. the final `:` in `2:67:`).
            if after < end && bytes[after] == CHAR_COLON {
                after += 1;
            }
        }
        skip_whitespace(bytes, after, end)
    } else {
        cursor
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
        let next = consume_optional_colon_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertBefore(Anchor { line: anchor.line }),
            next_index: next,
        });
    }

    // INS.POST N:
    if let Some(after_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, after_end, end), end)?;
        let next = consume_optional_colon_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertAfter(Anchor { line: anchor.line }),
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

fn scan_hunk_anchor(bytes: &[u8], start: usize, end: usize) -> Option<TargetScan> {
    let cursor = skip_whitespace(bytes, start, end);

    // SWAP.BLK N:
    if let Some(block_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, block_end, end), end)?;
        let next = consume_optional_colon_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::Block(Anchor { line: anchor.line }),
            next_index: next,
        });
    }

    // SWAP N..=M:
    if let Some(replace_end) = scan_keyword(bytes, cursor, end, HL_REPLACE_KEYWORD) {
        let range = scan_header_range(bytes, replace_end, end, true)?;
        let next = consume_optional_colon_with_hash(bytes, range.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::Replace(range.range),
            next_index: next,
        });
    }

    // DEL.BLK N (no colon)
    if let Some(del_block_end) = scan_keyword(bytes, cursor, end, HL_DELETE_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, del_block_end, end), end)?;
        let next = skip_whitespace(bytes, anchor.next_index, end);
        if next < end && bytes[next] == CHAR_COLON {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::DeleteBlock(Anchor { line: anchor.line }),
            next_index: next,
        });
    }

    // DEL N..=M (no colon)
    if let Some(delete_end) = scan_keyword(bytes, cursor, end, HL_DELETE_KEYWORD) {
        let range = scan_header_range(bytes, delete_end, end, true)?;
        let next = skip_whitespace(bytes, range.next_index, end);
        if next < end && bytes[next] == CHAR_COLON {
            return None;
        }
        return Some(TargetScan {
            target: BlockTarget::Delete(range.range),
            next_index: next,
        });
    }

    // INS.BLK.POST N:
    if let Some(iblk_end) = scan_keyword(bytes, cursor, end, HL_INSERT_AFTER_BLOCK_KEYWORD) {
        let anchor = scan_line_number(bytes, skip_whitespace(bytes, iblk_end, end), end)?;
        let next = consume_optional_colon_with_hash(bytes, anchor.next_index, end);
        return Some(TargetScan {
            target: BlockTarget::InsertAfterBlock(Anchor { line: anchor.line }),
            next_index: next,
        });
    }

    // INS.xxx
    if let Some(insert_end) = scan_keyword(bytes, cursor, end, HL_INSERT_KEYWORD) {
        return scan_insert_target(bytes, insert_end, end);
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
    let scan = scan_hunk_anchor(bytes, start, end)?;
    if scan.next_index != end {
        return None;
    }
    Some(scan.target)
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
    Some(HeaderResult {
        path: path.to_owned(),
        file_hash,
    })
}

struct HeaderResult {
    path: String,
    file_hash: Option<String>,
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
    }

    let lead = skip_whitespace(line.as_bytes(), 0, line.len());
    let is_hunk_lead = line[lead..].starts_with(HL_REPLACE_KEYWORD)
        || line[lead..].starts_with(HL_DELETE_KEYWORD)
        || line[lead..].starts_with(HL_INSERT_KEYWORD);
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
        try_parse_header(line).is_some()
    }

    pub fn is_envelope_marker(&self, line: &str) -> bool {
        marker_line_equals(line, BEGIN_PATCH_MARKER)
            || marker_line_equals(line, END_PATCH_MARKER)
            || marker_line_equals(line, ABORT_MARKER)
    }
}
