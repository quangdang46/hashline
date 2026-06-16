//! Hashline format primitives: sigils, separators, regex fragments, and
//! display helpers. These are the single source of truth for the parser, the
//! tokenizer, and CLI help text.

use crate::types::Cursor;

/** File-section header delimiters: `[path#hash]`. */
pub const HL_FILE_PREFIX: &str = "[";
pub const HL_FILE_SUFFIX: &str = "]";

/** Payload sigil for literal body rows. */
pub const HL_PAYLOAD_REPLACE: &str = "+";

/** Hunk-header keyword for concrete line replacement. */
pub const HL_REPLACE_KEYWORD: &str = "SWAP";
/** Hunk-header keyword for concrete line deletion. */
pub const HL_DELETE_KEYWORD: &str = "DEL";
/** Hunk-header keyword for insertion operations. */
pub const HL_INSERT_KEYWORD: &str = "INS";
/** Insert position keyword for inserting before a concrete line. */
pub const HL_INSERT_BEFORE: &str = "PRE";
/** Insert position keyword for inserting after a concrete line. */
pub const HL_INSERT_AFTER: &str = "POST";
/** Insert position keyword for inserting at the start of the file. */
pub const HL_INSERT_HEAD: &str = "HEAD";
/** Insert position keyword for inserting at the end of the file. */
pub const HL_INSERT_TAIL: &str = "TAIL";
/** Hunk-header keyword: `SWAP.BLK N:` resolves N to a tree-sitter block range and replaces its span. */
pub const HL_REPLACE_BLOCK_KEYWORD: &str = "SWAP.BLK";
/** Hunk-header keyword: `DEL.BLK N` resolves N to a tree-sitter block range and deletes its span. */
pub const HL_DELETE_BLOCK_KEYWORD: &str = "DEL.BLK";
/** Hunk-header keyword: `INS.BLK.POST N:` inserts after the last line of the tree-sitter block at N. */
pub const HL_INSERT_AFTER_BLOCK_KEYWORD: &str = "INS.BLK.POST";
pub const HL_HEADER_COLON: &str = ":";

/** Separator between a hashline file path and its opaque snapshot tag. */
pub const HL_FILE_HASH_SEP: &str = "#";

/** Separator between two line numbers in a range, e.g. `5..=10`. */
pub const HL_RANGE_SEP: &str = "..";

/** Separator between a line number and displayed line content. */
pub const HL_LINE_BODY_SEP: &str = ":";

/** Number of hex characters in a content-derived file-hash tag. */
pub const HL_FILE_HASH_LENGTH: usize = 4;

/** Representative file-hash tags for use in user-facing error messages. */
pub const HL_FILE_HASH_EXAMPLES: [&str; 3] = ["1A2B", "3C4D", "9F3E"];

/// Format a concrete replacement hunk header: `SWAP 5..=10:`
pub fn format_replace_header(start: usize, end: usize) -> String {
    format!("{HL_REPLACE_KEYWORD} {start}{HL_RANGE_SEP}{end}{HL_HEADER_COLON}")
}

/// Format a concrete deletion hunk header: `DEL 12` or `DEL 5..=10`
pub fn format_delete_header(start: usize, end: usize) -> String {
    if start == end {
        format!("{HL_DELETE_KEYWORD} {start}")
    } else {
        format!("{HL_DELETE_KEYWORD} {start}{HL_RANGE_SEP}{end}")
    }
}

/// Format an insertion hunk header for a cursor position.
pub fn format_insert_header(cursor: &Cursor) -> String {
    match cursor {
        Cursor::BeforeAnchor(anchor) => {
            format!("{HL_INSERT_KEYWORD}.{HL_INSERT_BEFORE} {}{HL_HEADER_COLON}", anchor.line)
        }
        Cursor::AfterAnchor(anchor) => {
            format!("{HL_INSERT_KEYWORD}.{HL_INSERT_AFTER} {}{HL_HEADER_COLON}", anchor.line)
        }
        Cursor::Bof => format!("{HL_INSERT_KEYWORD}.{HL_INSERT_HEAD}{HL_HEADER_COLON}"),
        Cursor::Eof => format!("{HL_INSERT_KEYWORD}.{HL_INSERT_TAIL}{HL_HEADER_COLON}"),
    }
}

/// Format a hashline section header: `[path#1A2B]`
pub fn format_hashline_header(path: &str, hash: &str) -> String {
    format!("{HL_FILE_PREFIX}{path}{HL_FILE_HASH_SEP}{hash}{HL_FILE_SUFFIX}")
}

/// Format a single numbered line as `LINE:TEXT`.
pub fn format_numbered_line(line_number: usize, line: &str) -> String {
    format!("{line_number}{HL_LINE_BODY_SEP}{line}")
}

/// Format file text with hashline-mode line-number prefixes for display.
pub fn format_numbered_lines(text: &str, start_line: usize) -> String {
    text.split('\n')
        .enumerate()
        .map(|(i, line)| format_numbered_line(start_line + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a comma-separated list of example anchors, quoted for inclusion
/// in error messages.
pub fn describe_anchor_examples(line_prefix: Option<&str>) -> String {
    let examples = match line_prefix {
        Some(prefix) => {
            vec![
                prefix.to_owned(),
                format!("{}2", &prefix[..prefix.len().saturating_sub(1)]),
                "7".to_owned(),
            ]
        }
        None => vec!["160".to_owned(), "42".to_owned(), "7".to_owned()],
    };
    examples
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ")
}
