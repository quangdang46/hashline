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
// MCP instructions — advertised during initialize so hosts auto-inject them
// into every agent session (issue #107).
// ---------------------------------------------------------------------------

const MCP_INSTRUCTIONS: &str = "\
Hashline provides hash-anchored file editing.
Read a file first to get stable anchors ([path#HASH], line:hash), then patch
by anchor. Prefer hashline for content-anchored edits over str_replace-style
tools: anchors survive nearby edits, and stale reads are rejected instead of
corrupting the file.

Workflow:
1. hashline read <file>        — get anchors
2. hashline patch <file> <op>  — edit by anchor (use stdin for multi-op)
3. If stale anchor error: re-read and retry with fresh anchors.

Commands: read, patch, find_block, write, remove_file, rename_file
Patch ops: SWAP, DEL, INS.POST, INS.PRE, INS.HEAD, INS.TAIL,
           SWAP.BLK, DEL.BLK, INS.BLK.POST, INS.BLK.PRE, CUT, PUT";

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
                name: "read".into(),
                description: "Read a file with [path#HASH] header and numbered lines".into(),
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
                name: "patch".into(),
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
                name: "write".into(),
                description:
                    "Write content to a file (creates new file or overwrites with --force)".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string"},
                        "content": {"type": "string", "description": "File content to write"},
                        "force": {"type": "boolean", "description": "Overwrite existing file if it exists"},
                        "json": {"type": "boolean", "description": "Output as JSON"}
                    },
                    "required": ["file", "content"]
                })),
            },
            ToolDefinition {
                name: "find_block".into(),
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
            ToolDefinition {
                name: "remove_file".into(),
                description: "Delete a file".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Path to the file to delete"}
                    },
                    "required": ["file"]
                })),
            },
            ToolDefinition {
                name: "rename_file".into(),
                description: "Rename (move) a file".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "src": {"type": "string", "description": "Source file path"},
                        "dst": {"type": "string", "description": "Destination file path"}
                    },
                    "required": ["src", "dst"]
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
    let entries = fc.lines_with_hashes();

    if json {
        let lines: Vec<Value> = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                serde_json::json!({
                    "n": i + 1,
                    "hash": hash::format_short_hash(entry.short_hash),
                    "content": entry.content,
                })
            })
            .collect();
        let output = serde_json::json!({
            "path": fc.path.display().to_string(),
            "hash": fc.hash,
            "lines": lines,
        });
        serde_json::to_string(&output).unwrap_or_default()
    } else {
        let mut out = format!("[{}#{}]\n", fc.path.display(), fc.hash);
        let mut hash_buf = [0u8; 2];
        for (i, entry) in entries.iter().enumerate() {
            hash::write_short_hash_bytes(&mut hash_buf, entry.short_hash);
            let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
            out.push_str(&format!("{}:{}|{}\n", i + 1, hash_str, entry.content));
        }
        out
    }
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
        fc.len()
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

    // If anchor is at indent 0 (e.g. a `def` statement), it IS the block header.
    if anchor_indent == 0 {
        let mut end = entries.len() - 1;
        for i in (anchor_index + 1)..entries.len() {
            let t = entries[i].content.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if leading_ws(&entries[i].content) <= anchor_indent {
                end = i.saturating_sub(1);
                break;
            }
        }
        return Ok((anchor_index, end));
    }

    // Indented anchor: scan backward for the block header (less-indented line)
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

fn handle_remove_file(file: &str, json: bool) -> String {
    let path = std::path::Path::new(file);
    if !path.exists() {
        return format!("Error: file not found: {file}");
    }
    match std::fs::remove_file(path) {
        Ok(_) => {
            if json {
                serde_json::json!({"success": true, "file": file}).to_string()
            } else {
                format!("Removed {file}")
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_rename_file(src: &str, dst: &str, json: bool) -> String {
    let src_path = std::path::Path::new(src);
    let dst_path = std::path::Path::new(dst);
    if !src_path.exists() {
        return format!("Error: source not found: {src}");
    }
    if dst_path.exists() {
        return format!("Error: destination already exists: {dst}");
    }
    if let Some(parent) = dst_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(src_path, dst_path) {
        Ok(_) => {
            if json {
                serde_json::json!({"success": true, "src": src, "dst": dst}).to_string()
            } else {
                format!("Renamed {src} -> {dst}")
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_patch(file: &str, patch_str: &str, dry_run: bool) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let (edits, warnings, _file_op, _aborted) = parse_patch(patch_str);

    // Surface warnings in the tool result so MCP clients see them
    // (Issue #93 — warnings are not just debug info for the terminal).
    // The first line of the response carries the outcome; warnings follow
    // as indented notes that are visible in both CLI and agent output.
    let mut warning_line = String::new();
    if !warnings.is_empty() {
        warning_line = format!(
            "\n  warnings:\n{}",
            warnings
                .iter()
                .map(|w| format!("    - {w}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    if edits.is_empty() {
        let base = if warnings.is_empty() {
            "Error: patch produced no edits — input was empty or all operations were rejected"
                .to_string()
        } else {
            let reason = &warnings[0];
            format!("Error: patch produced no edits — {reason}")
        };
        return format!("{base}{warning_line}");
    }

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
        format!("Dry-run result:\n{final_text}")
    } else {
        match crate::commands::common::fast_write(path, final_text.as_bytes()) {
            Ok(_) => format!("Patch applied.{warning_line}\n{}", handle_read(file, false)),
            Err(e) => format!("Error writing file: {e}"),
        }
    }
}

fn handle_write(file: &str, content: &str, force: bool, json: bool) -> String {
    let path = Path::new(file);

    // Check file exists — refuse unless --force
    if path.exists() && !force {
        return format!(
            "Error: target '{}' already exists — use force=true to overwrite",
            file
        );
    }

    let normalized = crate::normalize::normalize_to_lf(content);
    let bom_result = crate::normalize::strip_bom(&normalized);
    let write_content = bom_result.text;

    if let Err(e) = crate::commands::common::fast_write(path, write_content.as_bytes()) {
        return format!("Error: {e}");
    }

    match FileContent::load(path) {
        Ok(fc) => {
            let entries = fc.lines_with_hashes();

            if json {
                let lines: Vec<Value> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, entry)| {
                        serde_json::json!({
                            "n": i + 1,
                            "hash": hash::format_short_hash(entry.short_hash),
                            "content": entry.content,
                        })
                    })
                    .collect();
                serde_json::to_string(&serde_json::json!({
                    "success": true,
                    "path": fc.path.display().to_string(),
                    "hash": fc.hash,
                    "lines": lines,
                }))
                .unwrap_or_default()
            } else {
                let mut out = format!("[{}#{}]\n", fc.path.display(), fc.hash);
                let mut hash_buf = [0u8; 2];
                for (i, entry) in entries.iter().enumerate() {
                    hash::write_short_hash_bytes(&mut hash_buf, entry.short_hash);
                    let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
                    out.push_str(&format!("{}:{}|{}\n", i + 1, hash_str, entry.content));
                }
                out
            }
        }
        Err(e) => format!("Written successfully.\nError re-reading file: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "read" | "hashline_read" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(serde_json::json!({"content": [{"type": "text", "text": handle_read(file, json)}]}))
        }
        "patch" | "hashline_patch" => {
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
        "write" | "hashline_write" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("missing 'content'")?;
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_write(file, content, force, json)}]}),
            )
        }
        "find_block" | "hashline_find_block" => {
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
        "remove_file" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_remove_file(file, json)}]}),
            )
        }
        "rename_file" => {
            let src = args
                .get("src")
                .and_then(|v| v.as_str())
                .ok_or("missing 'src'")?;
            let dst = args
                .get("dst")
                .and_then(|v| v.as_str())
                .ok_or("missing 'dst'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(
                serde_json::json!({"content": [{"type": "text", "text": handle_rename_file(src, dst, json)}]}),
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
            let info = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "hashline",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": MCP_INSTRUCTIONS
            });
            session._server_info = Some(info.clone());
            Ok(info)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_includes_instructions() {
        let mut session = Session::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "initialize".into(),
            params: None,
            id: Some(json!(1)),
        };
        let resp = handle_request(&req, &mut session);
        let result = resp.result.unwrap();
        let instructions = result.get("instructions").and_then(|v| v.as_str());
        assert!(
            instructions.is_some(),
            "initialize response must include 'instructions' field"
        );
        let text = instructions.unwrap();
        assert!(text.contains("hashline"), "instructions should mention hashline");
        assert!(
            text.contains("read"),
            "instructions should mention the read-first workflow"
        );
        assert!(
            text.contains("str_replace"),
            "instructions should mention str_replace preference"
        );
        assert!(
            text.contains("stale"),
            "instructions should mention stale-read rejection"
        );
    }

    #[test]
    fn initialize_server_info_and_capabilities() {
        let mut session = Session::new();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "initialize".into(),
            params: None,
            id: Some(json!(1)),
        };
        let resp = handle_request(&req, &mut session);
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "hashline");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
    }
}
