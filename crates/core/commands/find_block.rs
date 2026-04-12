use std::io::Write;
use std::path::Path;

use serde::Serialize;

use crate::anchor::{parse_anchor, resolve};
use crate::cli::FindBlockCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: FindBlockCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let index = doc.build_index();
    let anchor = parse_anchor(&cmd.anchor)?;
    let resolved = resolve(&anchor, &doc, &index)?;
    let language = detect_language(&doc, resolved.index)?;
    let block = match language {
        BlockLanguage::Brace => find_brace_block(&doc, resolved.index)?,
        BlockLanguage::Indent => find_indent_block(&doc, resolved.index)?,
    };

    match ctx.output_mode() {
        OutputMode::Pretty => output::write_success_line(
            ctx,
            &format!(
                "Block: {}:{}..{}:{}  ({} lines — {})",
                block.start_line,
                crate::document::format_short_hash(doc.lines[block.start_index].short_hash),
                block.end_line,
                crate::document::format_short_hash(doc.lines[block.end_index].short_hash),
                block.line_count(),
                language.description(),
            ),
        )
        .map_err(LinehashError::from),
        OutputMode::Json => output::write_json_success(
            ctx,
            &BlockPayload {
                start: format!(
                    "{}:{}",
                    block.start_line,
                    crate::document::format_short_hash(doc.lines[block.start_index].short_hash)
                ),
                end: format!(
                    "{}:{}",
                    block.end_line,
                    crate::document::format_short_hash(doc.lines[block.end_index].short_hash)
                ),
                line_count: block.line_count(),
                language: language.name(),
            },
        )
        .map_err(LinehashError::from),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockLanguage {
    Brace,
    Indent,
}

impl BlockLanguage {
    fn name(self) -> &'static str {
        match self {
            BlockLanguage::Brace => "brace",
            BlockLanguage::Indent => "indent",
        }
    }

    fn description(self) -> &'static str {
        match self {
            BlockLanguage::Brace => "brace-balanced",
            BlockLanguage::Indent => "indent-delimited",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockRange {
    start_index: usize,
    end_index: usize,
    start_line: usize,
    end_line: usize,
}

impl BlockRange {
    fn line_count(self) -> usize {
        self.end_line - self.start_line + 1
    }
}

#[derive(Serialize)]
struct BlockPayload {
    start: String,
    end: String,
    line_count: usize,
    language: &'static str,
}

fn detect_language(doc: &Document, anchor_index: usize) -> Result<BlockLanguage, LinehashError> {
    if is_indent_extension(&doc.path) {
        return Ok(BlockLanguage::Indent);
    }
    if is_brace_extension(&doc.path) {
        return Ok(BlockLanguage::Brace);
    }

    let mut saw_brace = false;
    let mut saw_indent = false;
    for index in 0..doc.lines.len() {
        let line = doc.lines[index].content.as_str();
        if line.contains('{') || line.contains('}') {
            saw_brace = true;
        }
        if looks_like_indent_block_header(line)
            && next_nonblank_indent(doc, index)
                .is_some_and(|next| next > leading_indent_width(line))
        {
            saw_indent = true;
        }
    }

    match (saw_brace, saw_indent) {
        (true, false) => Ok(BlockLanguage::Brace),
        (false, true) => Ok(BlockLanguage::Indent),
        _ => Err(LinehashError::AmbiguousBlockLanguage {
            line_no: anchor_index + 1,
        }),
    }
}

fn find_brace_block(doc: &Document, anchor_index: usize) -> Result<BlockRange, LinehashError> {
    let mut stack: Vec<usize> = Vec::new();
    let mut blocks = Vec::new();
    let mut state = BraceScanState::default();

    for (line_index, line) in doc.lines.iter().enumerate() {
        for brace in code_braces(line.content.as_str(), &mut state) {
            match brace {
                BraceToken::Open => stack.push(line_index),
                BraceToken::Close => {
                    let Some(start_index) = stack.pop() else {
                        return Err(LinehashError::UnbalancedBlock {
                            line_no: anchor_index + 1,
                        });
                    };
                    blocks.push(BlockRange {
                        start_index,
                        end_index: line_index,
                        start_line: start_index + 1,
                        end_line: line_index + 1,
                    });
                }
            }
        }
    }

    if !stack.is_empty() {
        return Err(LinehashError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    blocks
        .into_iter()
        .filter(|block| block.start_index <= anchor_index && anchor_index <= block.end_index)
        .min_by_key(|block| (block.start_index, usize::MAX - block.end_index))
        .ok_or(LinehashError::UnbalancedBlock {
            line_no: anchor_index + 1,
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BraceToken {
    Open,
    Close,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BraceScanState {
    block_comment_depth: usize,
    string: Option<StringState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StringState {
    DoubleQuoted,
    SingleQuoted,
    Backtick,
    RawString { hashes: usize },
}

fn code_braces(line: &str, state: &mut BraceScanState) -> Vec<BraceToken> {
    let bytes = line.as_bytes();
    let mut braces = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if state.block_comment_depth > 0 {
            if starts_with(bytes, index, b"/*") {
                state.block_comment_depth += 1;
                index += 2;
                continue;
            }
            if starts_with(bytes, index, b"*/") {
                state.block_comment_depth -= 1;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if let Some(string) = state.string.as_mut() {
            match string {
                StringState::DoubleQuoted => {
                    index = scan_quoted(bytes, index, b'"');
                    if index <= bytes.len() && bytes.get(index.saturating_sub(1)) == Some(&b'"') {
                        state.string = None;
                    }
                    continue;
                }
                StringState::SingleQuoted => {
                    index = scan_quoted(bytes, index, b'\'');
                    if index <= bytes.len() && bytes.get(index.saturating_sub(1)) == Some(&b'\'') {
                        state.string = None;
                    }
                    continue;
                }
                StringState::Backtick => {
                    index = scan_backtick(bytes, index);
                    if index <= bytes.len() && bytes.get(index.saturating_sub(1)) == Some(&b'`') {
                        state.string = None;
                    }
                    continue;
                }
                StringState::RawString { hashes } => {
                    index = scan_raw_string(bytes, index, *hashes);
                    if raw_string_closed(bytes, index, *hashes) {
                        state.string = None;
                    }
                    continue;
                }
            }
        }

        if starts_with(bytes, index, b"//") {
            break;
        }
        if starts_with(bytes, index, b"/*") {
            state.block_comment_depth = 1;
            index += 2;
            continue;
        }
        if let Some(hashes) = raw_string_hashes(bytes, index) {
            state.string = Some(StringState::RawString { hashes });
            index += hashes + 2;
            continue;
        }

        match bytes[index] {
            b'"' => {
                state.string = Some(StringState::DoubleQuoted);
                index += 1;
            }
            b'\'' if is_probable_single_quoted_literal(bytes, index) => {
                state.string = Some(StringState::SingleQuoted);
                index += 1;
            }
            b'`' => {
                state.string = Some(StringState::Backtick);
                index += 1;
            }
            b'{' => {
                braces.push(BraceToken::Open);
                index += 1;
            }
            b'}' => {
                braces.push(BraceToken::Close);
                index += 1;
            }
            _ => index += 1,
        }
    }

    braces
}

fn starts_with(bytes: &[u8], index: usize, needle: &[u8]) -> bool {
    bytes
        .get(index..index + needle.len())
        .is_some_and(|slice| slice == needle)
}

fn scan_quoted(bytes: &[u8], mut index: usize, quote: u8) -> usize {
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        index += 1;
        if bytes[index - 1] == quote {
            break;
        }
    }
    index
}

fn scan_backtick(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        index += 1;
        if bytes[index - 1] == b'`' {
            break;
        }
    }
    index
}

fn raw_string_hashes(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) != Some(&b'r') {
        return None;
    }
    let mut cursor = index + 1;
    while bytes.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'"') {
        Some(cursor - index - 1)
    } else {
        None
    }
}

fn scan_raw_string(bytes: &[u8], mut index: usize, hashes: usize) -> usize {
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        let mut matched = 0;
        while matched < hashes && bytes.get(cursor) == Some(&b'#') {
            matched += 1;
            cursor += 1;
        }
        if matched == hashes {
            return cursor;
        }
        index += 1;
    }
    index
}

fn raw_string_closed(bytes: &[u8], index: usize, hashes: usize) -> bool {
    if index == 0 || index > bytes.len() {
        return false;
    }
    let quote_index = index - hashes - 1;
    bytes.get(quote_index) == Some(&b'"')
        && bytes
            .get(quote_index + 1..index)
            .is_some_and(|slice| slice.iter().all(|&byte| byte == b'#'))
}

fn is_probable_single_quoted_literal(bytes: &[u8], index: usize) -> bool {
    let Some(next) = bytes.get(index + 1) else {
        return false;
    };
    if next.is_ascii_alphabetic() || *next == b'_' {
        return false;
    }

    let mut cursor = index + 1;
    while cursor < bytes.len() && cursor <= index + 6 {
        if bytes[cursor] == b'\\' {
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b'\'' {
            return true;
        }
        cursor += 1;
    }
    false
}

fn find_indent_block(doc: &Document, anchor_index: usize) -> Result<BlockRange, LinehashError> {
    let mut candidate = None;
    for index in 0..=anchor_index {
        let line = doc.lines[index].content.as_str();
        if is_blank(line) || !looks_like_indent_block_header(line) {
            continue;
        }
        let header_indent = leading_indent_width(line);
        let Some(next_indent) = next_nonblank_indent(doc, index) else {
            continue;
        };
        if next_indent <= header_indent {
            continue;
        }
        let end_index = indent_block_end(doc, index, header_indent);
        if candidate.is_none() && index <= anchor_index && anchor_index <= end_index {
            candidate = Some(BlockRange {
                start_index: index,
                end_index,
                start_line: index + 1,
                end_line: end_index + 1,
            });
        }
    }

    candidate.ok_or(LinehashError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })
}

fn indent_block_end(doc: &Document, header_index: usize, header_indent: usize) -> usize {
    let mut end_index = header_index;
    for index in header_index + 1..doc.lines.len() {
        let line = doc.lines[index].content.as_str();
        if is_blank(line) {
            end_index = index;
            continue;
        }
        if leading_indent_width(line) <= header_indent {
            break;
        }
        end_index = index;
    }
    end_index
}

fn next_nonblank_indent(doc: &Document, from_index: usize) -> Option<usize> {
    doc.lines
        .iter()
        .skip(from_index + 1)
        .map(|line| line.content.as_str())
        .find(|line| !is_blank(line))
        .map(leading_indent_width)
}

fn leading_indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| matches!(ch, ' ' | '\t'))
        .count()
}

fn looks_like_indent_block_header(line: &str) -> bool {
    line.trim_end().ends_with(':')
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_indent_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("py" | "yaml" | "yml")
    )
}

fn is_brace_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rs" | "js" | "ts" | "jsx" | "tsx" | "java" | "c" | "cc" | "cpp" | "h" | "hpp" | "go")
    )
}

#[cfg(test)]
mod tests {
    use super::{code_braces, find_brace_block, BraceScanState, BraceToken};
    use crate::document::Document;
    use std::path::Path;

    #[test]
    fn code_braces_ignores_rust_strings_and_comments() {
        let mut state = BraceScanState::default();
        assert_eq!(
            code_braces("let s = \"{\";", &mut state),
            Vec::<BraceToken>::new()
        );
        assert_eq!(code_braces("// }", &mut state), Vec::<BraceToken>::new());
        assert_eq!(
            code_braces("struct ParsedStatus {", &mut state),
            vec![BraceToken::Open]
        );
        assert_eq!(code_braces("}", &mut state), vec![BraceToken::Close]);
    }

    #[test]
    fn find_brace_block_tolerates_non_code_braces_before_anchor() {
        let doc = Document::from_str(
            Path::new("demo.rs"),
            "fn noisy() {\n    let _ = \"{not a block\";\n}\n\nstruct ParsedStatus {\n    value: u32,\n}\n",
        )
        .unwrap();

        let block = find_brace_block(&doc, 4).unwrap();
        assert_eq!(block.start_line, 5);
        assert_eq!(block.end_line, 7);
    }
}
