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
    // Resolve patch content: `-` reads stdin, `@path` reads file, otherwise use literal.
    let patch_content = resolve_patch_content(&cmd.patch)?;

    let fc = FileContent::load(&cmd.file)?;
    let text = &fc.normalized;
    let (edits, warnings) = parse_patch(&patch_content);

    // Surface parser warnings even when no edits were produced so callers
    // can tell a syntactically-broken patch from an empty one.
    for w in &warnings {
        eprintln!("warning: {w}");
    }

    if edits.is_empty() {
        return Err(HashlineError::EmptyPatch);
    }

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
        // Show a unified-diff-alike snippet instead of the entire file.
        let original_text = &fc.normalized;
        let diff_lines = format_diff(original_text, &final_text);
        for dl in &diff_lines {
            writeln!(ctx.stdout(), "{dl}")?;
        }
        return Ok(());
    }

    if cmd.safe {
        crate::commands::common::atomic_write(&cmd.file, final_text.as_bytes())?;
    } else {
        crate::commands::common::fast_write(&cmd.file, final_text.as_bytes())?;
    }

    // Structured JSON output for agent integration.
    if cmd.json {
        use crate::hash::format_short_hash;
        let new_entries: Vec<serde_json::Value> = final_text
            .split('\n')
            .filter(|l| !l.is_empty() || final_text.ends_with('\n'))
            .enumerate()
            .map(|(i, content)| {
                let short = crate::hash::short_hash_value(content);
                serde_json::json!({
                    "line": i + 1,
                    "hash": format_short_hash(short),
                    "content": content,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "success": true,
            "file": cmd.file.display().to_string(),
            "edits_applied": edits.len(),
            "lines": new_entries,
        });
        writeln!(ctx.stdout(), "{}", serde_json::to_string(&payload)?)?;
    }

    Ok(())
}

/// Resolve the `<PATCH>` argument: `-` reads stdin, `@path` reads file, otherwise literal.
fn resolve_patch_content(patch: &str) -> Result<String, HashlineError> {
    if patch == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(HashlineError::Io)?;
        return Ok(buf);
    }
    if let Some(path_str) = patch.strip_prefix('@') {
        let path = Path::new(path_str);
        return std::fs::read_to_string(path).map_err(|e| {
            HashlineError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("failed to read patch file '{}': {e}", path.display()),
            ))
        });
    }
    Ok(patch.to_owned())
}

/// Produce a minimal unified-diff snippet showing only changed lines,
/// suitable for dry-run review. Uses a simple LCS-based shortest-edit
/// path that correctly handles insertions and deletions that shift
/// subsequent line indices. Loosely follows `diff -u` style but omits
/// the timestamp header.
fn format_diff(original: &str, final_text: &str) -> Vec<String> {
    let left: Vec<&str> = if original.is_empty() {
        vec![]
    } else {
        original.split('\n').collect()
    };
    let right: Vec<&str> = if final_text.is_empty() {
        vec![]
    } else {
        final_text.split('\n').collect()
    };
    if left == right {
        return vec!["(no changes)".into()];
    }
    // Compute LCS table (Wagner-Fischer).
    let m = left.len();
    let n = right.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if left[i - 1] == right[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    // Backtrack to produce edit operations.
    let mut ops: Vec<(usize, &'static str, &str)> = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && left[i - 1] == right[j - 1] {
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push((j, "+", right[j - 1]));
            j -= 1;
        } else {
            ops.push((i, "-", left[i - 1]));
            i -= 1;
        }
    }
    ops.reverse();
    let mut out: Vec<String> = Vec::new();
    out.push("@@ -- ++ @@".into());
    for (_, tag, line) in &ops {
        out.push(format!("{tag}{line}"));
    }
    out
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

    // ---- Empty-patch detection (fixes #58) ----

    /// Build a synthetic `Edit::Insert` for empty-patch assertions.
    fn parse_only(text: &str) -> Vec<crate::types::Edit> {
        let (edits, _warnings) = parse_patch(text);
        edits
    }

    #[test]
    fn parse_patch_empty_string_yields_no_edits() {
        let edits = parse_only("");
        assert!(edits.is_empty(), "expected zero edits for empty patch");
    }

    #[test]
    fn parse_patch_unparseable_garbage_yields_no_edits() {
        let edits = parse_only("this is not a hashline patch\nborked\n!!");
        assert!(edits.is_empty(), "expected zero edits for garbage patch");
    }

    #[test]
    fn parse_patch_hash_suffix_yields_real_swap() {
        // From issue #56: `SWAP 2:67:` used to be silently rejected by the
        // tokenizer, producing zero edits. After the fix, the hash suffix
        // is consumed and the SWAP produces a replacement insert + delete.
        let edits = parse_only("SWAP 2:67:\n+REPLACED");
        assert_eq!(
            edits.len(),
            2,
            "expected 1 insert + 1 delete, got {edits:?}"
        );
        match &edits[0] {
            crate::types::Edit::Insert {
                cursor: crate::types::Cursor::BeforeAnchor(a),
                text,
                mode,
                ..
            } => {
                assert_eq!(a.line, 2);
                assert_eq!(text, "REPLACED");
                assert!(matches!(mode, Some(crate::types::InsertMode::Replacement)));
            }
            other => panic!("expected BeforeAnchor insert, got {other:?}"),
        }
        match &edits[1] {
            crate::types::Edit::Delete { anchor, .. } => {
                assert_eq!(anchor.line, 2);
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn parse_patch_hash_suffix_on_range() {
        // Range form: `SWAP 2..3:67:` should also accept the hash suffix
        // and produce 2 inserts + 2 deletes.
        let edits = parse_only("SWAP 2..3:67:\n+AAA\n+BBB");
        assert_eq!(
            edits.len(),
            4,
            "expected 2 inserts + 2 deletes, got {edits:?}"
        );
    }
}
