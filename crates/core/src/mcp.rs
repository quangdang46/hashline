//! MCP (Model Context Protocol) server for hashline.
//!
//! Runs over stdio and exposes hashline operations as MCP tools.
//! Supports: initialize, tools/list, tools/call for all hashline operations.

use std::io::{self, BufRead, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::McpCmd;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::hash;
use crate::normalize::{LineEnding, detect_line_ending, restore_line_endings};
use crate::parser::parse_patch;

/// Split normalized text into lines, discarding the trailing empty segment
/// that split('\n') produces when the text ends with '\n'.
fn split_text(text: &str) -> (Vec<String>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let trailing_newline = text.ends_with('\n');
    let parts: Vec<&str> = text.split('\n').collect();
    let mut lines: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    if trailing_newline && lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    (lines, trailing_newline)
}

fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    if trailing_newline {
        lines.join("\n") + "\n"
    } else {
        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default)]
    pub id: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ---------------------------------------------------------------------------
// MCP protocol types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ToolList {
    tools: Vec<ToolDefinition>,
}

#[derive(Serialize)]
struct ToolDefinition {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "inputSchema")]
    input_schema: Option<Value>,
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

fn tool_list() -> ToolList {
    ToolList {
        tools: vec![
            ToolDefinition {
                name: "hashline_read".into(),
                description:
                    "Read a file with [path#HASH] header and numbered lines".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Path to the file"},
                        "json": {"type": "boolean", "description": "Output as JSON"}
                    },
                    "required": ["file"]
                })),
            },
            ToolDefinition {
                name: "hashline_patch".into(),
                description: "Apply a hashline patch (SWAP, DEL, INS.* operations)".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string"},
                        "patch": {"type": "string", "description": "Hashline patch string"},
                        "dry_run": {"type": "boolean"}
                    },
                    "required": ["file", "patch"]
                })),
            },
            ToolDefinition {
                name: "hashline_find_block".into(),
                description: "Find a likely structural block around an anchor".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string"},
                        "anchor": {"type": "string", "description": "Line:hash anchor"}
                    },
                    "required": ["file", "anchor"]
                })),
            },
        ],
    }
}

fn handle_read(file: &str, json: bool) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };

    if json {
        let raw_lines = fc.lines();
        let lines: Vec<Value> = raw_lines
            .iter()
            .enumerate()
            .filter(|(i, line)| {
                !(line.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline)
            })
            .map(|(i, line)| serde_json::json!({"n": i + 1, "content": line}))
            .collect();
        let output = serde_json::json!({
            "path": fc.path.display().to_string(),
            "hash": fc.hash,
            "lines": lines,
        });
        serde_json::to_string(&output).unwrap_or_default()
    } else {
        let mut out = format!("[{}#{}]\n", fc.path.display(), fc.hash);
        let lines = fc.lines();
        let count = lines.len();
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() && i == count - 1 && fc.trailing_newline {
                continue;
            }
            out.push_str(&format!("{}|{}\n", i + 1, line));
        }
        out
    }
}

fn handle_index(file: &str) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();
    let mut out = format!("[{}#{}]\n", fc.path.display(), fc.hash);
    for (i, entry) in entries.iter().enumerate() {
        let hash = hash::format_short_hash(entry.short_hash);
        // Skip trailing empty line
        if entry.content.is_empty() && i == entries.len() - 1 && fc.trailing_newline {
            continue;
        }
        out.push_str(&format!("{}:{}\n", i + 1, hash));
    }
    out
}

fn handle_annotate(file: &str, query: &str, use_regex: bool) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();
    let mut results = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        if entry.content.is_empty() && i == entries.len() - 1 && fc.trailing_newline {
            continue;
        }
        let matched = if use_regex {
            regex::Regex::new(query)
                .map(|re| re.is_match(&entry.content))
                .unwrap_or(false)
        } else {
            entry.content.contains(query)
        };
        if matched {
            let hash = hash::format_short_hash(entry.short_hash);
            results.push(format!("{}:{}|{}", i + 1, hash, entry.content));
        }
    }

    if results.is_empty() {
        format!("query '{}' not found in {}", query, file)
    } else if results.len() == 1 {
        results.into_iter().next().unwrap()
    } else {
        results.join("\n")
    }
}

fn handle_grep(file: &str, pattern: &str, _invert: bool) -> String {
    handle_annotate(file, pattern, false) // Simple grep = literal substring match
}

fn handle_find_block(file: &str, anchor_str: &str) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();

    let parsed = match crate::anchor::parse_anchor(anchor_str) {
        Ok(a) => a,
        Err(e) => return format!("Error: {e}"),
    };
    let resolved = match crate::anchor::resolve(&parsed, &fc) {
        Ok(r) => r,
        Err(e) => return format!("Error: {e}"),
    };

    let anchor_index = resolved.index;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = match extension {
        "rs" => "Rust",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "go" => "Go",
        "rb" => "Ruby",
        _ => "Unknown",
    };

    // Find block boundaries
    let block_result: Option<(usize, usize)> = match extension {
        "rs" | "js" | "ts" | "go" | "java" | "c" | "cpp" | "h" | "cs" => {
            find_brace_block(&entries, anchor_index, extension).ok()
        }
        "py" => find_indent_block(&entries, anchor_index).ok(),
        "rb" => find_ruby_block(&entries, anchor_index).ok(),
        _ => find_brace_block(&entries, anchor_index, extension)
            .ok()
            .or_else(|| find_indent_block(&entries, anchor_index).ok()),
    };

    let (start, end) = block_result.unwrap_or((anchor_index, anchor_index));

    let mut out = format!(
        "File: {}  ({} lines)\nLanguage: {language}\n",
        fc.path.display(),
        entries.len()
    );
    for i in start..=end {
        let entry = &entries[i];
        let hash = hash::format_short_hash(entry.short_hash);
        out.push_str(&format!("{}:{}|{}\n", i + 1, hash, entry.content));
    }
    out
}

fn find_brace_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    ext: &str,
) -> Result<(usize, usize), ()> {
    let pairs = find_brace_pairs(entries, ext);
    for (s, e) in pairs.iter().rev() {
        if *s <= anchor_index && *e >= anchor_index {
            return Ok((*s, *e));
        }
    }
    Err(())
}

fn find_brace_pairs(entries: &[crate::document::LineEntry], _ext: &str) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
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

fn find_indent_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
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
    let start = match start {
        Some(s) => s,
        None => return Err(()),
    };
    let si = leading_ws(&entries[start].content);
    let mut end = entries.len() - 1;
    for i in (start + 1)..entries.len() {
        let t = entries[i].content.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if leading_ws(&entries[i].content) <= si {
            end = i.saturating_sub(1);
            break;
        }
    }
    Ok((start, end))
}

fn find_ruby_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let mut depth: isize = 0;
    let mut start = None;
    for i in (0..=anchor_index).rev() {
        let t = entries[i].content.trim();
        let ec = if t == "end" { 1 } else { 0 };
        let oc = if t.starts_with("def ")
            || t.starts_with("class ")
            || t.starts_with("module ")
            || t.starts_with("do ")
            || t.starts_with("if ")
            || t.starts_with("unless ")
        {
            1
        } else {
            0
        };
        depth += ec as isize;
        depth -= oc as isize;
        if oc > 0 && depth <= 0 {
            start = Some(i);
            break;
        }
    }
    let start = match start {
        Some(s) => s,
        None => return Err(()),
    };
    depth = 0;
    for i in start..entries.len() {
        let t = entries[i].content.trim();
        let oc = if t.starts_with("def ")
            || t.starts_with("class ")
            || t.starts_with("module ")
            || t.starts_with("do ")
            || t.starts_with("if ")
            || t.starts_with("unless ")
        {
            1
        } else {
            0
        };
        let ec = if t == "end" { 1 } else { 0 };
        depth += oc as isize;
        depth -= ec as isize;
        if i > start && depth <= 0 && t == "end" {
            return Ok((start, i));
        }
    }
    Err(())
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

fn handle_verify(file: &str, anchors: &[String]) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();
    let mut results = Vec::new();
    let mut any_fail = false;

    for anchor_str in anchors {
        let parsed = match crate::anchor::parse_anchor(anchor_str) {
            Ok(a) => a,
            Err(e) => {
                any_fail = true;
                results.push(format!("{}: parse error - {e}", anchor_str));
                continue;
            }
        };
        match crate::anchor::resolve_with_entries(&parsed, &entries, &fc) {
            Ok(r) => {
                let h = hash::format_short_hash(entries[r.index].short_hash);
                results.push(format!(
                    "{} -> line {}:{} | {}",
                    anchor_str, r.line_no, h, entries[r.index].content
                ));
            }
            Err(e) => {
                any_fail = true;
                results.push(format!("{}: FAIL - {e}", anchor_str));
            }
        }
    }

    if any_fail {
        format!("FAILURES:\n{}", results.join("\n"))
    } else {
        format!("OK:\n{}", results.join("\n"))
    }
}

fn handle_edit(file: &str, anchor_str: &str, content: &str) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();

    let (start_line, end_line) = if let Ok(range) = crate::anchor::parse_range(anchor_str) {
        let start = match crate::anchor::resolve_with_entries(&range.start, &entries, &fc) {
            Ok(r) => r.index,
            Err(e) => return format!("Error: {e}"),
        };
        let end = match crate::anchor::resolve_with_entries(&range.end, &entries, &fc) {
            Ok(r) => r.index,
            Err(e) => return format!("Error: {e}"),
        };
        (start, end)
    } else {
        let parsed = match crate::anchor::parse_anchor(anchor_str) {
            Ok(a) => a,
            Err(e) => return format!("Error: {e}"),
        };
        let resolved = match crate::anchor::resolve_with_entries(&parsed, &entries, &fc) {
            Ok(r) => r,
            Err(e) => return format!("Error: {e}"),
        };
        (resolved.index, resolved.index)
    };

    let (mut lines, _trailing_newline) = split_text(&fc.normalized);

    if end_line >= lines.len() {
        return format!("Error: line {} out of range", end_line + 1);
    }

    let new_content_lines: Vec<String> = content.split('\n').map(|s| s.to_string()).collect();
    let num_old = end_line - start_line + 1;
    for _ in 0..num_old {
        lines.remove(start_line);
    }
    for (k, line_text) in new_content_lines.iter().enumerate() {
        lines.insert(start_line + k, line_text.clone());
    }

    let result = join_lines(&lines, fc.trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    match crate::commands::common::atomic_write(path, final_text.as_bytes()) {
        Ok(_) => format!(
            "Edited lines {}-{}.\n{}",
            start_line + 1,
            end_line + 1,
            handle_read(file, false)
        ),
        Err(e) => format!("Error writing file: {e}"),
    }
}

fn handle_insert(file: &str, anchor_str: &str, content: &str, before: bool) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();

    let parsed = match crate::anchor::parse_anchor(anchor_str) {
        Ok(a) => a,
        Err(e) => return format!("Error: {e}"),
    };
    let resolved = match crate::anchor::resolve_with_entries(&parsed, &entries, &fc) {
        Ok(r) => r,
        Err(e) => return format!("Error: {e}"),
    };

    let (mut lines, _) = split_text(&fc.normalized);

    let insert_line = if before {
        resolved.index
    } else {
        resolved.index + 1
    };

    for (k, line_text) in content.split('\n').enumerate() {
        let pos = (insert_line + k).min(lines.len());
        lines.insert(pos, line_text.to_string());
    }

    let result = join_lines(&lines, fc.trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    match crate::commands::common::atomic_write(path, final_text.as_bytes()) {
        Ok(_) => format!(
            "Inserted after line {}.\n{}",
            resolved.line_no,
            handle_read(file, false)
        ),
        Err(e) => format!("Error writing file: {e}"),
    }
}

fn handle_delete(file: &str, anchor_str: &str) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let entries = fc.lines_with_hashes();

    let (start_line, end_line) = if let Ok(range) = crate::anchor::parse_range(anchor_str) {
        let start = match crate::anchor::resolve_with_entries(&range.start, &entries, &fc) {
            Ok(r) => r.index,
            Err(e) => return format!("Error: {e}"),
        };
        let end = match crate::anchor::resolve_with_entries(&range.end, &entries, &fc) {
            Ok(r) => r.index,
            Err(e) => return format!("Error: {e}"),
        };
        (start, end)
    } else {
        let parsed = match crate::anchor::parse_anchor(anchor_str) {
            Ok(a) => a,
            Err(e) => return format!("Error: {e}"),
        };
        let resolved = match crate::anchor::resolve_with_entries(&parsed, &entries, &fc) {
            Ok(r) => r,
            Err(e) => return format!("Error: {e}"),
        };
        (resolved.index, resolved.index)
    };

    let (mut lines, _) = split_text(&fc.normalized);

    let num_del = end_line.saturating_sub(start_line).saturating_add(1);
    for _ in 0..num_del.min(lines.len().saturating_sub(start_line)) {
        if start_line < lines.len() {
            lines.remove(start_line);
        }
    }

    let result = join_lines(&lines, fc.trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    match crate::commands::common::atomic_write(path, final_text.as_bytes()) {
        Ok(_) => format!(
            "Deleted lines {}-{}.\n{}",
            start_line + 1,
            end_line + 1,
            handle_read(file, false)
        ),
        Err(e) => format!("Error writing file: {e}"),
    }
}

fn handle_patch(file: &str, patch_str: &str, dry_run: bool) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let (edits, _warnings) = parse_patch(patch_str);

    let (mut lines, _) = split_text(&fc.normalized);

    let entries = fc.lines_with_hashes();
    if let Err(e) = crate::commands::patch::apply_edits(&mut lines, &entries, path, &edits) {
        return format!("Error: {e}");
    }

    let result = join_lines(&lines, fc.trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    if dry_run {
        format!("Dry-run result:\n{}", final_text)
    } else {
        match crate::commands::common::atomic_write(path, final_text.as_bytes()) {
            Ok(_) => format!("Patch applied.\n{}", handle_read(file, false)),
            Err(e) => format!("Error writing file: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "hashline_read" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(serde_json::json!({"content": [{"type": "text", "text": handle_read(file, json)}]}))
        }
        "hashline_index" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            Ok(serde_json::json!({"content": [{"type": "text", "text": handle_index(file)}]}))
        }
        "hashline_annotate" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or("missing 'query'")?;
            let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_annotate(file, query, use_regex)}]}),
            )
        }
        "hashline_grep" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let pattern = args
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or("missing 'pattern'")?;
            let invert = args
                .get("invert")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_grep(file, pattern, invert)}]}),
            )
        }
        "hashline_find_block" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let anchor = args
                .get("anchor")
                .and_then(|v| v.as_str())
                .ok_or("missing 'anchor'")?;
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_find_block(file, anchor)}]}),
            )
        }
        "hashline_verify" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let anchors: Vec<String> = args
                .get("anchors")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .ok_or("missing 'anchors'")?;
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_verify(file, &anchors)}]}),
            )
        }
        "hashline_edit" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let anchor = args
                .get("anchor")
                .and_then(|v| v.as_str())
                .ok_or("missing 'anchor'")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("missing 'content'")?;
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_edit(file, anchor, content)}]}),
            )
        }
        "hashline_insert" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let anchor = args
                .get("anchor")
                .and_then(|v| v.as_str())
                .ok_or("missing 'anchor'")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("missing 'content'")?;
            let before = args
                .get("before")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_insert(file, anchor, content, before)}]}),
            )
        }
        "hashline_delete" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let anchor = args
                .get("anchor")
                .and_then(|v| v.as_str())
                .ok_or("missing 'anchor'")?;
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_delete(file, anchor)}]}),
            )
        }
        "hashline_patch" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let patch = args
                .get("patch")
                .and_then(|v| v.as_str())
                .ok_or("missing 'patch'")?;
            let dry_run = args
                .get("dry_run")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_patch(file, patch, dry_run)}]}),
            )
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

pub struct Session {
    _server_info: Option<Value>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session { _server_info: None }
    }
}

// ---------------------------------------------------------------------------
// Main request handler
// ---------------------------------------------------------------------------

pub fn handle_request(request: &JsonRpcRequest, session: &mut Session) -> JsonRpcResponse {
    let id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" => {
            session._server_info = Some(serde_json::json!({"protocolVersion": "2024-11-05"}));
            Ok(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}}
            }))
        }
        "tools/list" => Ok(serde_json::to_value(tool_list()).unwrap_or_default()),
        "tools/call" => {
            let params = request
                .params
                .as_ref()
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            call_tool(tool_name, &arguments)
        }
        "ping" => Ok(serde_json::json!({})),
        _ => Err(format!("unknown method: {}", request.method)),
    };

    match result {
        Ok(data) => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(data),
            error: None,
        },
        Err(msg) => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: msg,
                data: None,
            }),
        },
    }
}

pub fn write_error<W: Write>(
    writer: &mut W,
    _id: Option<Value>,
    code: i32,
    message: &str,
    _data: Option<Value>,
) -> io::Result<()> {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: None,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data: None,
        }),
    };
    serde_json::to_writer(&mut *writer, &response)?;
    writeln!(&mut *writer)
}

pub fn dispatch_tool(
    tool_name: &str,
    params: &Option<Value>,
    _session: &mut Session,
) -> Result<Value, JsonRpcError> {
    let args = params
        .as_ref()
        .and_then(|p| p.as_object())
        .and_then(|o| o.get("arguments"))
        .cloned()
        .unwrap_or(serde_json::json!({}));
    call_tool(tool_name, &args).map_err(|msg| JsonRpcError {
        code: -32601,
        message: msg,
        data: None,
    })
}

/// Run the MCP server (CLI entry point).
pub fn run(_cmd: McpCmd) -> Result<(), HashlineError> {
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    let mut reader = io::BufReader::new(stdin);
    let mut writer = io::BufWriter::new(stdout);
    let mut session = Session::new();

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(HashlineError::Io)?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                write_error(
                    &mut writer,
                    None,
                    -32700,
                    &format!("parse error: {e}"),
                    None,
                )
                .map_err(HashlineError::Io)?;
                continue;
            }
        };

        if request.id.is_none() {
            continue;
        }
        let response = handle_request(&request, &mut session);
        serde_json::to_writer(&mut writer, &response).map_err(HashlineError::Json)?;
        writer.write_all(b"\n").map_err(HashlineError::Io)?;
        writer.flush().map_err(HashlineError::Io)?;
    }

    Ok(())
}
