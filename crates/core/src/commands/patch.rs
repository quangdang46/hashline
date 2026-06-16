use std::io::Write;
use std::path::Path;

use crate::cli::PatchCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::normalize::{LineEnding, detect_line_ending, restore_line_endings};
use crate::parser::parse_patch;
use crate::types::{BlockMode, Cursor, Edit, InsertMode};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: PatchCmd,
) -> Result<(), HashlineError> {
    let fc = FileContent::load(&cmd.file)?;
    let text = &fc.normalized;
    let (edits, _warnings) = parse_patch(&cmd.patch);

    // Split on newlines. Drop the trailing empty segment that split('\n')
    // produces when a file ends with '\n' — we add it back on join.
    let mut lines: Vec<String> = split_normalized(text);
    let had_trailing_newline = fc.trailing_newline;

    let entries = fc.lines_with_hashes();
    apply_edits(&mut lines, &entries, &cmd.file, &edits)?;

    let result = if had_trailing_newline && !lines.is_empty() {
        lines.join("\n") + "\n"
    } else if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    if cmd.dry_run {
        writeln!(ctx.stdout(), "{}", final_text)?;
    } else {
        crate::commands::common::atomic_write(&cmd.file, final_text.as_bytes())?;
    }

    Ok(())
}

fn split_normalized(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = text.split('\n').collect();
    // Drop trailing empty from split when file ends with '\n'
    if text.ends_with('\n') && parts.last() == Some(&"") {
        parts.pop();
    }
    parts.iter().map(|s| s.to_string()).collect()
}

/// Apply parsed edits to a mutable lines vector.
pub fn apply_edits(
    lines: &mut Vec<String>,
    entries: &[crate::document::LineEntry],
    path: &Path,
    edits: &[Edit],
) -> Result<(), HashlineError> {
    let mut i = 0;
    use std::collections::HashMap;
    let mut insert_count: HashMap<usize, usize> = HashMap::new();

    while i < edits.len() {
        match &edits[i] {
            // ---- SWAP N..=M: -------------------------------------------------
            Edit::Insert {
                mode: Some(InsertMode::Replacement),
                cursor: Cursor::BeforeAnchor(start_anchor),
                ..
            } => {
                let anchor_line = start_anchor.line;

                let mut replacement_texts: Vec<String> = Vec::new();
                let mut j = i;
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Insert {
                            mode: Some(InsertMode::Replacement),
                            cursor: Cursor::BeforeAnchor(a),
                            text,
                            ..
                        } if a.line == anchor_line => {
                            replacement_texts.push(text.clone());
                            j += 1;
                        }
                        _ => break,
                    }
                }

                let mut delete_lines: Vec<usize> = Vec::new();
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Delete { anchor, .. } => {
                            delete_lines.push(anchor.line);
                            j += 1;
                        }
                        _ => break,
                    }
                }

                let num_new = replacement_texts.len();
                let num_old = delete_lines.len();

                if num_new > 0 {
                    let start_idx = anchor_line.wrapping_sub(1);
                    let remove_end = (start_idx + num_old).min(lines.len());
                    for _ in start_idx..remove_end {
                        lines.remove(start_idx);
                    }
                    for (k, text) in replacement_texts.iter().enumerate() {
                        lines.insert(start_idx + k, text.clone());
                    }
                }

                i = j;
            }

            // ---- DEL N / DEL N..=M -------------------------------------------
            Edit::Delete { .. } => {
                let mut del_lines: Vec<usize> = Vec::new();
                let mut j = i;
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Delete { anchor, .. } => {
                            del_lines.push(anchor.line);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                del_lines.sort_by(|a, b| b.cmp(a));
                for line in &del_lines {
                    let idx = line.wrapping_sub(1);
                    if idx < lines.len() {
                        lines.remove(idx);
                    }
                }
                i = j;
            }

            // ---- INS.PRE / INS.POST / INS.HEAD / INS.TAIL --------------------
            Edit::Insert { cursor, text, .. } => {
                let base_line = match cursor {
                    Cursor::BeforeAnchor(a) => a.line.wrapping_sub(1),
                    Cursor::AfterAnchor(a) => a.line,
                    Cursor::Bof => 0,
                    Cursor::Eof => lines.len(),
                };
                let offset = insert_count.get(&base_line).copied().unwrap_or(0);
                insert_count.insert(base_line, offset + 1);
                let pos = if base_line + offset <= lines.len() {
                    base_line + offset
                } else {
                    lines.len()
                };
                lines.insert(pos, text.clone());
                i += 1;
            }

            // ---- SWAP.BLK N: / DEL.BLK N / INS.BLK.POST N: -----------------
            Edit::Block {
                anchor,
                payloads,
                mode,
                ..
            } => {
                let line_no = anchor.line;
                let anchor_index = line_no.wrapping_sub(1);
                if anchor_index >= entries.len() {
                    i += 1;
                    continue;
                }

                // Resolve the syntactic block starting at line_no
                let (block_start, block_end) = resolve_block_span(entries, anchor_index, path)?;

                match mode {
                    None if payloads.is_empty() => {
                        // DEL.BLK N
                        for _ in block_start..=block_end.min(lines.len().saturating_sub(1)) {
                            lines.remove(block_start);
                        }
                    }
                    None => {
                        // SWAP.BLK N: replace the entire block (header + body) with payload
                        let num_old = block_end - block_start + 1;
                        for _ in 0..num_old.min(lines.len()) {
                            if block_start < lines.len() {
                                lines.remove(block_start);
                            }
                        }
                        for (k, payload) in payloads.iter().enumerate() {
                            lines.insert(block_start + k, payload.clone());
                        }
                    }
                    Some(BlockMode::InsertAfter) => {
                        // INS.BLK.POST N: insert after the last line of the block
                        let insert_pos = (block_end + 1).min(lines.len());
                        for (k, payload) in payloads.iter().enumerate() {
                            lines.insert(insert_pos + k, payload.clone());
                        }
                    }
                }

                i += 1;
            }
        }
    }
    Ok(())
}

/// Resolve a 1-indexed anchor line to the syntactic block span (0-based inclusive).
///
/// Uses language detection from file extension, then brace-matching,
/// indentation-based, or Ruby `end`-based block finding.
fn resolve_block_span(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    path: &Path,
) -> Result<(usize, usize), HashlineError> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    match extension {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp" | "cs"
        | "kt" | "kts" | "swift" | "scala" | "dart" | "zig" | "m" | "mm" => {
            find_brace_block(entries, anchor_index, extension)
        }
        "py" | "verse" => find_python_block(entries, anchor_index),
        "rb" => find_ruby_block(entries, anchor_index),
        _ => find_brace_block(entries, anchor_index, extension)
            .or_else(|_| find_indent_block(entries, anchor_index)),
    }
    .map_err(|_| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })
}

// ---------------------------------------------------------------------------
// Brace-balanced block finding
// ---------------------------------------------------------------------------

fn find_brace_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    ext: &str,
) -> Result<(usize, usize), ()> {
    let pairs = find_brace_pairs(entries, ext);
    pairs
        .iter()
        .filter(|(start, end)| *start <= anchor_index && *end >= anchor_index)
        .max_by_key(|(start, _)| *start)
        .copied()
        .ok_or(())
}

fn find_brace_pairs(entries: &[crate::document::LineEntry], _ext: &str) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let line_comment: &[u8] = b"//";
    let mut in_block_comment = false;

    for (line_idx, entry) in entries.iter().enumerate() {
        let bytes = entry.content.as_bytes();
        let mut i = 0;
        let mut in_sq = false;
        let mut in_dq = false;
        let mut esc = false;

        while i < bytes.len() {
            if esc {
                esc = false;
                i += 1;
                continue;
            }
            if (in_sq || in_dq) && bytes[i] == b'\\' {
                esc = true;
                i += 1;
                continue;
            }
            if in_block_comment {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i..].starts_with(line_comment) {
                break;
            }
            if !in_sq && !in_dq && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                in_block_comment = true;
                i += 2;
                continue;
            }
            if in_sq && bytes[i] == b'\'' {
                in_sq = false;
                i += 1;
                continue;
            }
            if in_dq && bytes[i] == b'"' {
                in_dq = false;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i] == b'\'' {
                in_sq = true;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i] == b'"' {
                in_dq = true;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && !in_block_comment {
                if bytes[i] == b'{' {
                    stack.push(line_idx);
                } else if bytes[i] == b'}' {
                    if let Some(s) = stack.pop() {
                        pairs.push((s, line_idx));
                    }
                }
            }
            i += 1;
        }
    }
    pairs
}

// ---------------------------------------------------------------------------
// Indentation-based block finding
// ---------------------------------------------------------------------------

fn find_indent_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    // If anchor IS at indent 0, it's a block header itself.
    if anchor_indent == 0 {
        find_block_from_header(entries, anchor_index, &["//"])
    } else {
        find_block_from_body(entries, anchor_index, &["//", "#"])
    }
}

fn find_python_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    if anchor_indent == 0 {
        find_block_from_header(entries, anchor_index, &["#"])
    } else {
        find_block_from_body(entries, anchor_index, &["#"])
    }
}

/// Block header line (indent 0): scan forward to find same-or-less indent.
fn find_block_from_header(
    entries: &[crate::document::LineEntry],
    start: usize,
    _comments: &[&str],
) -> Result<(usize, usize), ()> {
    let si = leading_ws(&entries[start].content);
    let mut end = entries.len() - 1;
    for i in (start + 1)..entries.len() {
        if leading_ws(&entries[i].content) <= si {
            end = i.saturating_sub(1);
            break;
        }
    }
    Ok((start, end))
}

/// Block body line (indented): scan backward for header, then forward for end.
fn find_block_from_body(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    _comments: &[&str],
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    let mut start = None;
    for i in (0..anchor_index).rev() {
        if entries[i].content.trim().is_empty() {
            continue;
        }
        if leading_ws(&entries[i].content) < anchor_indent {
            start = Some(i);
            break;
        }
    }
    let start = start.ok_or(())?;
    let si = leading_ws(&entries[start].content);
    let mut end = entries.len() - 1;
    for i in (start + 1)..entries.len() {
        let t = entries[i].content.trim();
        if t.is_empty() {
            continue;
        }
        if leading_ws(&entries[i].content) <= si {
            end = i.saturating_sub(1);
            break;
        }
    }
    Ok((start, end))
}

// ---------------------------------------------------------------------------
// Ruby ...end block finding
// ---------------------------------------------------------------------------

const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ",
];

fn find_ruby_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let mut depth: isize = 0;
    let mut start = None;
    for i in (0..=anchor_index).rev() {
        let trimmed = entries[i].content.trim();
        let ec = if trimmed == "end" { 1 } else { 0 };
        let oc = ruby_opener_count(trimmed);
        depth += ec as isize;
        depth -= oc as isize;
        if oc > 0 && depth <= 0 {
            start = Some(i);
            break;
        }
    }
    let start = start.ok_or(())?;
    depth = 0;
    for i in start..entries.len() {
        let trimmed = entries[i].content.trim();
        let oc = ruby_opener_count(trimmed);
        let ec = if trimmed == "end" { 1 } else { 0 };
        depth += oc as isize;
        depth -= ec as isize;
        if i > start && depth <= 0 && trimmed == "end" {
            return Ok((start, i));
        }
        if i == start && depth <= 0 {
            return Ok((start, i));
        }
    }
    Err(())
}

fn ruby_opener_count(trimmed: &str) -> usize {
    for opener in RUBY_OPENERS {
        if trimmed.starts_with(opener) {
            return 1;
        }
    }
    0
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_text_ext(original: &str, patch_text: &str, ext: &str) -> String {
        let (edits, _warnings) = parse_patch(patch_text);
        let mut lines: Vec<String> = if original.is_empty() {
            Vec::new()
        } else {
            original.split('\n').map(|s| s.to_string()).collect()
        };
        let path_str = format!("test.{ext}");
        let entries_with_content: Vec<crate::document::LineEntry> = lines
            .iter()
            .map(|s| crate::document::LineEntry {
                content: s.clone(),
                short_hash: crate::hash::short_hash_value(s),
            })
            .collect();
        apply_edits(
            &mut lines,
            &entries_with_content,
            std::path::Path::new(&path_str),
            &edits,
        )
        .expect("edit should succeed");
        lines.join("\n")
    }

    fn apply_text(original: &str, patch_text: &str) -> String {
        apply_text_ext(original, patch_text, "rs")
    }

    #[test]
    fn test_swap_single_line() {
        let result = apply_text("line1\nline2\nline3", "SWAP 2:\n+replaced2");
        assert_eq!(result, "line1\nreplaced2\nline3");
    }

    #[test]
    fn test_swap_range() {
        let result = apply_text("line1\nline2\nline3\nline4", "SWAP 2..3:\n+x\n+y");
        assert_eq!(result, "line1\nx\ny\nline4");
    }

    #[test]
    fn test_delete_single() {
        let result = apply_text("line1\nline2\nline3", "DEL 2");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_delete_range() {
        let result = apply_text("line1\nline2\nline3\nline4", "DEL 2..3");
        assert_eq!(result, "line1\nline4");
    }

    #[test]
    fn test_insert_post() {
        let result = apply_text("line1\nline2", "INS.POST 1:\n+inserted");
        assert_eq!(result, "line1\ninserted\nline2");
    }

    #[test]
    fn test_insert_pre() {
        let result = apply_text("line1\nline2", "INS.PRE 2:\n+inserted");
        assert_eq!(result, "line1\ninserted\nline2");
    }

    #[test]
    fn test_insert_head() {
        let result = apply_text("line1\nline2", "INS.HEAD:\n+header");
        assert_eq!(result, "header\nline1\nline2");
    }

    #[test]
    fn test_insert_tail() {
        let result = apply_text("line1\nline2", "INS.TAIL:\n+footer");
        assert_eq!(result, "line1\nline2\nfooter");
    }

    #[test]
    fn test_multiple_insert_post() {
        let result = apply_text("line1\nline2", "INS.POST 1:\n+a\n+b");
        assert_eq!(result, "line1\na\nb\nline2");
    }

    #[test]
    fn test_swap_without_body_reduces_to_delete_range() {
        let result = apply_text("line1\nline2\nline3", "SWAP 2:");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_empty_original_swapped() {
        let result = apply_text("", "INS.HEAD:\n+newline");
        assert_eq!(result, "newline");
    }

    #[test]
    fn test_patch_with_header() {
        let result = apply_text("line1\nline2\nline3", "[file.txt#abcd]\nDEL 2");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_swap_then_insert() {
        let result = apply_text("a\nb\nc", "SWAP 2:\n+x\nINS.TAIL:\n+y");
        assert_eq!(result, "a\nx\nc\ny");
    }

    // ---- Block operation tests ----

    #[test]
    fn test_swap_block_rust_function() {
        // Block: fn hello() { ... } spans lines 1..6 (0-indexed 0..5)
        let original =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let patch = "SWAP.BLK 1:\n+fn replaced() {\n+    // new body\n+}\n";
        let result = apply_text(original, patch);
        // The old block (6 lines) is replaced with 3 replacement lines
        assert_eq!(result, "fn replaced() {\n    // new body\n}\n");
    }

    #[test]
    fn test_swap_block_inner() {
        // Anchor at line 3 (if true { ... }) should replace the if-block, not the outer fn
        let original =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let patch = "SWAP.BLK 3:\n+if false {\n+        // nothing\n+    }\n";
        let result = apply_text(original, patch);
        assert_eq!(
            result,
            "fn hello() {\n    let x = 1;\nif false {\n        // nothing\n    }\n}\n"
        );
    }

    #[test]
    fn test_delete_block_rust() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "DEL.BLK 1";
        let result = apply_text(original, patch);
        assert_eq!(result, "");
    }

    #[test]
    fn test_insert_after_block_rust() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "INS.BLK.POST 1:\n+fn world() {\n+    let y = 2;\n+}\n";
        let result = apply_text(original, patch);
        assert_eq!(
            result,
            "fn hello() {\n    let x = 1;\n}\nfn world() {\n    let y = 2;\n}\n"
        );
    }

    #[test]
    fn test_swap_block_python() {
        let original = "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        let result = apply_text_ext(original, "SWAP.BLK 1:\n+def hi():\n+    pass\n", "py");
        assert_eq!(result, "def hi():\n    pass\n");
    }
}
