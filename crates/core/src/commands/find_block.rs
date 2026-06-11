use std::io::Write;

use crate::cli::FindBlockCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash::write_short_hash_bytes;

/// Result JSON payload for find_block.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FindBlockPayload {
    pub file: String,
    pub line_count: usize,
    pub language: Option<String>,
    pub block_lines: Vec<IndexLineView>,
}

/// A single display line inside the block payload.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct IndexLineView {
    pub n: usize,
    pub hash: String,
    pub content: String,
}

/// CLI entry point.
pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: FindBlockCmd,
) -> Result<(), HashlineError> {
    let doc = Document::load(&cmd.file)?;
    let payload = find_block_payload(&doc, &cmd.anchor)?;

    if cmd.json {
        crate::output::write_json_success(ctx, &payload)?;
    } else {
        writeln!(
            ctx.stdout(),
            "File: {}  ({} lines)",
            payload.file,
            payload.line_count
        )?;
        if let Some(ref lang) = payload.language {
            writeln!(ctx.stdout(), "Language: {lang}")?;
        }
        let mut hash_buf = [0u8; 2];
        for line in &payload.block_lines {
            write_short_hash_bytes(&mut hash_buf, line.hash.parse::<u8>().unwrap_or(0));
            let hash_str = std::str::from_utf8(&hash_buf).unwrap_or("??");
            writeln!(ctx.stdout(), "{}:{}|{}", line.n, hash_str, line.content)?;
        }
    }

    Ok(())
}

/// Core logic: resolve anchor, detect language, find block.
pub fn find_block_payload(
    doc: &Document,
    anchor_str: &str,
) -> Result<FindBlockPayload, HashlineError> {
    let index = doc.build_index();
    let parsed = crate::anchor::parse_anchor(anchor_str)?;
    let resolved = crate::anchor::resolve(&parsed, doc, &index)?;
    let anchor_index = resolved.index;

    let extension = doc
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    let (language, block_start, block_end) = match extension {
        "py" => find_python_block(doc, anchor_index)?,
        "verse" => find_python_block(doc, anchor_index)?,
        "rb" => find_ruby_block(doc, anchor_index)?,
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp" | "cs"
        | "kt" | "kts" | "swift" | "scala" | "dart" | "zig" | "m" | "mm" => {
            let (s, e) = find_brace_block(doc, anchor_index, extension)?;
            (language_for_extension(extension), s, e)
        }
        _ => {
            // Best-effort: try indentation-based first, then brace-balanced
            match find_indent_block(doc, anchor_index) {
                Ok((s, e)) => (Some("Unknown".into()), s, e),
                Err(_) => {
                    let (s, e) = find_brace_block(doc, anchor_index, extension)?;
                    (Some("Unknown".into()), s, e)
                }
            }
        }
    };

    if block_start >= doc.lines.len() || block_end >= doc.lines.len() || block_start > block_end {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    let block_lines: Vec<IndexLineView> = (block_start..=block_end)
        .map(|i| {
            let line = &doc.lines[i];
            IndexLineView {
                n: i + 1,
                hash: crate::hash::format_short_hash(line.short_hash),
                content: line.content.to_string(),
            }
        })
        .collect();

    Ok(FindBlockPayload {
        file: doc.path.display().to_string(),
        line_count: doc.len(),
        language,
        block_lines,
    })
}

fn language_for_extension(ext: &str) -> Option<String> {
    match ext {
        "rs" => Some("Rust".into()),
        "py" => Some("Python".into()),
        "js" => Some("JavaScript".into()),
        "ts" => Some("TypeScript".into()),
        "tsx" => Some("TSX".into()),
        "jsx" => Some("JSX".into()),
        "go" => Some("Go".into()),
        "rb" => Some("Ruby".into()),
        "verse" => Some("Verse".into()),
        "java" => Some("Java".into()),
        "c" => Some("C".into()),
        "cpp" | "hpp" => Some("C++".into()),
        "h" => Some("C/C++ Header".into()),
        "cs" => Some("C#".into()),
        "kt" | "kts" => Some("Kotlin".into()),
        "swift" => Some("Swift".into()),
        "scala" => Some("Scala".into()),
        "dart" => Some("Dart".into()),
        "zig" => Some("Zig".into()),
        "m" => Some("Objective-C".into()),
        "mm" => Some("Objective-C++".into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Brace-balanced block finding (Rust, JS, TS, Go, Java, C, C++, etc.)
// ---------------------------------------------------------------------------
//
// Strategy: scan the entire doc once with a brace-pair stack, tracking
// string/comment state across lines. Then find the innermost brace pair
// that encloses the anchor.

fn find_brace_block(
    doc: &Document,
    anchor_index: usize,
    ext: &str,
) -> Result<(usize, usize), HashlineError> {
    let pairs = find_brace_pairs(doc, ext);

    // Find the innermost pair enclosing anchor_index.
    // Since pairs are inserted in order of closing `}`, we want
    // the one with the maximum start index that still encloses.
    let enclosing = pairs
        .iter()
        .filter(|(start, end)| *start <= anchor_index && *end >= anchor_index)
        .max_by_key(|(start, _)| *start)
        .copied()
        .ok_or_else(|| HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        })?;

    Ok(enclosing)
}

/// Scan the document once and return all matched `{` .. `}` pairs.
/// Tracks string literal and comment boundaries across lines.
fn find_brace_pairs(doc: &Document, ext: &str) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    let line_comment: &[u8] = if ext == "py" || ext == "rb" {
        b"#"
    } else {
        b"//"
    };
    let use_block_comments = ext != "py" && ext != "rb";

    let mut in_block_comment = false;

    for (line_idx, line) in doc.lines.iter().enumerate() {
        let bytes = line.content.as_bytes();
        let mut i = 0;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut prev_escape = false;

        while i < bytes.len() {
            // Track escape sequences within string literals
            if prev_escape {
                prev_escape = false;
                i += 1;
                continue;
            }
            if (in_single_quote || in_double_quote) && bytes[i] == b'\\' {
                prev_escape = true;
                i += 1;
                continue;
            }

            if in_block_comment {
                // Look for closing `*/`
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            // Line comment start (outside strings)
            if !in_single_quote && !in_double_quote && bytes[i..].starts_with(line_comment) {
                break; // rest of this line is a comment
            }

            // Block comment start (C-family)
            if use_block_comments
                && !in_single_quote
                && !in_double_quote
                && i + 1 < bytes.len()
                && bytes[i] == b'/'
                && bytes[i + 1] == b'*'
            {
                in_block_comment = true;
                i += 2;
                continue;
            }

            // Closing string delimiter
            if in_single_quote && bytes[i] == b'\'' {
                in_single_quote = false;
                i += 1;
                continue;
            }
            if in_double_quote && bytes[i] == b'"' {
                in_double_quote = false;
                i += 1;
                continue;
            }

            // Opening string delimiter
            if !in_single_quote && !in_double_quote && bytes[i] == b'\'' {
                in_single_quote = true;
                i += 1;
                continue;
            }
            if !in_single_quote && !in_double_quote && bytes[i] == b'"' {
                in_double_quote = true;
                i += 1;
                continue;
            }

            // Count braces
            if !in_single_quote && !in_double_quote && !in_block_comment {
                if bytes[i] == b'{' {
                    stack.push(line_idx);
                } else if bytes[i] == b'}' {
                    if let Some(start) = stack.pop() {
                        pairs.push((start, line_idx));
                    }
                }
            }

            i += 1;
        }
    }

    pairs
}

// ---------------------------------------------------------------------------
// Generic indentation-based block finding (Verse, or any unknown language)
// ---------------------------------------------------------------------------

/// Try to find an indentation-based block. Returns the same shape as
/// `find_python_block`: block header line at lower indent, block body
/// following.
fn find_indent_block(doc: &Document, anchor_index: usize) -> Result<(usize, usize), HashlineError> {
    let anchor_line = &doc.lines[anchor_index];
    let anchor_indent = leading_whitespace(anchor_line.content.as_ref());

    // Scan backward to find a line with less indentation (the block header).
    let mut start: Option<usize> = None;

    for i in (0..anchor_index).rev() {
        let line = &doc.lines[i];
        if line.content.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(line.content.as_ref());
        if indent < anchor_indent {
            start = Some(i);
            break;
        }
    }

    let start = start.ok_or_else(|| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })?;

    // Scan forward to find end of block: first non-empty line
    // with indentation <= start indent, minus one.
    let start_indent = leading_whitespace(doc.lines[start].content.as_ref());
    let mut end = doc.lines.len() - 1;

    for i in (start + 1)..doc.lines.len() {
        let line = &doc.lines[i];
        let trimmed = line.content.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let indent = leading_whitespace(line.content.as_ref());
        if indent <= start_indent {
            end = i.saturating_sub(1);
            break;
        }
    }

    Ok((start, end))
}

// ---------------------------------------------------------------------------
// Python indentation-based block finding
// ---------------------------------------------------------------------------

fn find_python_block(
    doc: &Document,
    anchor_index: usize,
) -> Result<(Option<String>, usize, usize), HashlineError> {
    let anchor_line = &doc.lines[anchor_index];
    let anchor_indent = leading_whitespace(anchor_line.content.as_ref());

    // Scan backward to find a line with less indentation (the block header).
    let mut start: Option<usize> = None;

    for i in (0..anchor_index).rev() {
        let line = &doc.lines[i];
        if line.content.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(line.content.as_ref());
        if indent < anchor_indent {
            start = Some(i);
            break;
        }
    }

    let start = start.ok_or_else(|| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })?;

    // Scan forward to find end of block: first non-empty, non-comment line
    // with indentation <= start indent, minus one.
    let start_indent = leading_whitespace(doc.lines[start].content.as_ref());
    let mut end = doc.lines.len() - 1;

    for i in (start + 1)..doc.lines.len() {
        let line = &doc.lines[i];
        let trimmed = line.content.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_whitespace(line.content.as_ref());
        if indent <= start_indent {
            end = i.saturating_sub(1);
            break;
        }
    }

    Ok((Some("Python".into()), start, end))
}

// ---------------------------------------------------------------------------
// Ruby ...end block finding
// ---------------------------------------------------------------------------

/// Ruby block-opening keywords (with trailing space or pipe for block params).
const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ", "case\n",
];

fn find_ruby_block(
    doc: &Document,
    anchor_index: usize,
) -> Result<(Option<String>, usize, usize), HashlineError> {
    // Scan backward to find block opener.
    let mut depth: isize = 0;
    let mut start: Option<usize> = None;

    for i in (0..=anchor_index).rev() {
        let trimmed = doc.lines[i].content.trim().to_string();

        let end_count = if trimmed == "end" { 1 } else { 0 };
        let open_count = ruby_opener_count(&trimmed);

        depth += end_count as isize;
        depth -= open_count as isize;

        if open_count > 0 && depth < 0 {
            start = Some(i);
            break;
        }

        // Also detect if we've found the opener at depth 0
        if open_count > 0 && depth == 0 {
            start = Some(i);
            break;
        }
    }

    let start = start.ok_or_else(|| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })?;

    // Scan forward to find matching `end`.
    depth = 0;
    let mut end: Option<usize> = None;

    for i in start..doc.lines.len() {
        let trimmed = doc.lines[i].content.trim().to_string();

        let open_count = ruby_opener_count(&trimmed);
        let end_count = if trimmed == "end" { 1 } else { 0 };

        // For `do` with inline block params (not `do |x|` after the do keyword)
        depth += open_count as isize;
        depth -= end_count as isize;

        if i > start && depth <= 0 && trimmed == "end" {
            end = Some(i);
            break;
        }
        if i == start && depth <= 0 {
            end = Some(i);
            break;
        }
    }

    let end = end.ok_or_else(|| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })?;

    Ok((Some("Ruby".into()), start, end))
}

fn ruby_opener_count(trimmed: &str) -> usize {
    for opener in RUBY_OPENERS {
        if trimmed.starts_with(opener) {
            return 1;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn leading_whitespace(content: &str) -> usize {
    content.len() - content.trim_start().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::hash::format_short_hash;
    use std::path::Path;

    fn anchor_for(line: usize, content: &str) -> String {
        let doc = Document::from_str(Path::new("_temp"), content).unwrap();
        let hash = format_short_hash(doc.lines[line.saturating_sub(1)].short_hash);
        format!("{line}:{hash}")
    }

    #[test]
    fn test_brace_pairs_simple() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let pairs = find_brace_pairs(&doc, "rs");
        assert_eq!(pairs.len(), 2, "expected 2 brace pairs, got {pairs:?}");
        // Inner if-block pair: { at line 2 (0-indexed), } at line 4
        assert_eq!(pairs[0], (2, 4));
        // Outer function pair: { at line 0, } at line 5
        assert_eq!(pairs[1], (0, 5));
    }

    #[test]
    fn test_brace_pairs_go_block() {
        let content = "func main() {\n\tif true {\n\t\tfmt.Println(\"ok\")\n\t}\n}\n";
        let doc = Document::from_str(Path::new("test.go"), content).unwrap();
        let pairs = find_brace_pairs(&doc, "go");
        assert_eq!(pairs.len(), 2, "expected 2 brace pairs, got {pairs:?}");
        assert_eq!(pairs[0], (1, 3));
        assert_eq!(pairs[1], (0, 4));
    }

    #[test]
    fn test_brace_pairs_with_block_comment() {
        let content = "fn hello() {\n    /* {\n}\n*/\n    let x = 1;\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let pairs = find_brace_pairs(&doc, "rs");
        // Pairs should be: (0, 5) for function, no pair for the brace inside comment
        assert_eq!(pairs.len(), 1, "expected 1 brace pair, got {pairs:?}");
        assert_eq!(pairs[0], (0, 5));
    }

    #[test]
    fn test_find_brace_block_resolve() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        // Anchor at line 3 (1-indexed) = "    if true {" (0-indexed 2)
        // Should find the if-block: start=2, end=4
        let (s, e) = find_brace_block(&doc, 2, "rs").unwrap();
        assert_eq!(s, 2, "expected start=2 (if-block)");
        assert_eq!(e, 4, "expected end=4 (if-block)");
    }

    #[test]
    fn test_brace_block_rust_function() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(3, content)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Rust"));
        // The if-block: lines 3..5
        assert_eq!(payload.block_lines.len(), 3);
        assert!(payload.block_lines[0].content.contains("if true"));
        assert_eq!(payload.block_lines[2].content, "    }");
    }

    #[test]
    fn test_brace_block_rust_outer_function() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(2, content)).unwrap();
        // The function block: lines 1..6
        assert_eq!(payload.block_lines.len(), 6);
        assert!(payload.block_lines[0].content.contains("fn hello()"));
        assert_eq!(payload.block_lines[5].content, "}");
    }

    #[test]
    fn test_python_block() {
        let content = "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        let doc = Document::from_str(Path::new("test.py"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(4, content)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Python"));
        // The enclosing block for the print line is the if-block:
        // "    if True:" + "        print('ok')" = 2 lines
        assert_eq!(payload.block_lines.len(), 2);
        assert_eq!(payload.block_lines[0].n, 3);
        assert!(payload.block_lines[0].content.contains("if True"));
    }

    #[test]
    fn test_python_outer_block() {
        let content = "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        let doc = Document::from_str(Path::new("test.py"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(2, content)).unwrap();
        assert_eq!(payload.block_lines.len(), 5);
        assert!(payload.block_lines[0].content.contains("def hello()"));
        assert_eq!(payload.block_lines[4].content, "    return x");
    }

    #[test]
    fn test_error_missing_anchor() {
        let doc = Document::from_str(Path::new("test.rs"), "fn a() {}\n").unwrap();
        let result = find_block_payload(&doc, "99:ff");
        assert!(result.is_err());
    }

    #[test]
    fn test_error_unbalanced_block() {
        let doc = Document::from_str(Path::new("test.rs"), "fn a() {\n  x\n").unwrap();
        let result = find_block_payload(&doc, "1:ff");
        assert!(result.is_err());
    }

    #[test]
    fn test_brace_detection_skips_strings() {
        let content = "fn hello() {\n    let s = \"{\";\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(2, content)).unwrap();
        assert_eq!(payload.block_lines.len(), 3);
        assert_eq!(payload.block_lines[2].content, "}");
    }

    #[test]
    fn test_ruby_block() {
        let content = "def hello\n  x = 1\n  if true\n    puts 'ok'\n  end\nend\n";
        let doc = Document::from_str(Path::new("test.rb"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(2, content)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Ruby"));
        assert_eq!(payload.block_lines.len(), 6);
        assert!(payload.block_lines[0].content.contains("def hello"));
        assert_eq!(payload.block_lines[5].content, "end");
    }

    #[test]
    fn test_brace_block_skips_block_comment() {
        let content = "fn hello() {\n    /* {\n}\n*/\n    let x = 1;\n}\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(5, content)).unwrap();
        assert_eq!(payload.block_lines.len(), 6);
        assert_eq!(payload.block_lines[5].content, "}");
    }

    #[test]
    fn test_js_block() {
        let content =
            "function foo() {\n  let x = 1;\n  if (true) {\n    console.log('ok');\n  }\n}\n";
        let doc = Document::from_str(Path::new("test.js"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(4, content)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("JavaScript"));
        assert_eq!(payload.block_lines.len(), 3);
        assert!(payload.block_lines[0].content.contains("if (true)"));
    }

    #[test]
    fn test_go_block() {
        let content = "func main() {\n\tif true {\n\t\tfmt.Println(\"ok\")\n\t}\n}\n";
        let doc = Document::from_str(Path::new("test.go"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(3, content)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Go"));
        assert_eq!(payload.block_lines.len(), 3);
        assert!(payload.block_lines[0].content.contains("if true"));
    }

    #[test]
    fn test_inline_block() {
        let content = "fn simple() { let x = 1; }\n";
        let doc = Document::from_str(Path::new("test.rs"), content).unwrap();
        let payload = find_block_payload(&doc, &anchor_for(1, content)).unwrap();
        assert_eq!(payload.block_lines.len(), 1);
        assert!(payload.block_lines[0].content.contains("fn simple()"));
    }
}
