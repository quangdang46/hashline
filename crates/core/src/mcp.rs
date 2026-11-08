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
Use hashline tools for hash-anchored edits rather than str_replace-style matching.
Workflow:
1. Call read with {\"file\":\"path\"} and inspect the content.
2. Call patch with the same file and a textual patch string. Put multiple
   operations in that string, not CLI commands or a structured edits array.
3. Verify the result and inspect any warnings. For another edit, use the full
   fresh listing requested with return_updated_anchors=true, or call read again.

Critical rules:
- Copy 1-based N:hh anchors from the latest read; never invent or adjust hashes.
  The [path#HASH] header is a file tag, not a line anchor. Hash validation is
  limited to supplied anchors, not every line or the whole file.
- Prefix every payload line with + and preserve indentation after it. Do not
  include read-output prefixes or repeat unchanged boundary lines.
- Batch independent, non-overlapping edits using the same pre-edit coordinates.
  Inspect block boundaries with find_block before using block operations.
- On stale anchors or invalid targets, read again, reassess, and rebuild the
  patch; never bypass the error by stripping hashes.

Tools: read, patch, write, find_block, remove_file, rename_file.
Use write for new files, patch for existing-file edits. See patch's parameter
schema for grammar, payload escaping, MCP examples, results and write limits.";

const MCP_PATCH_FORMAT: &str = r#"Textual operations separated by newlines. Headers use 1-based line numbers; copy N:hh from read for hash validation at N. A trailing colon introduces payload on following lines.

Operations (headers below are followed by payload on separate lines unless noted):
- Replace one original line, even when the payload has multiple lines:
  SWAP N:hh:
  +replacement
- Replace the inclusive original range N through M (N <= M):
  SWAP N:hh..M:
  +first replacement line
  +second replacement line
  The hash validates N only, not M or interior lines. Use N:hh..M, not two hash-qualified endpoints. Bare N..M is accepted but has no hash protection.
- Insert before/after N: INS.PRE N:hh: / INS.POST N:hh:, followed by payload.
- Insert at the start/end: INS.HEAD: / INS.TAIL:, followed by payload; no anchor.
- Delete one line/an inclusive range: DEL N:hh / DEL N:hh..M; no payload. A range hash validates only its start.
- Replace/delete/insert before/after the enclosing block: SWAP.BLK N:hh: / DEL.BLK N:hh / INS.BLK.PRE N:hh: / INS.BLK.POST N:hh:. All except DEL.BLK take payload on following lines. Bare N is also accepted without hash validation. Boundaries use language-dependent brace/indent/end heuristics, not tree-sitter; find_block is a preview, not a guarantee of identical resolution. Prefer separate calls for block edits that interact with other changes.
- Move an inclusive range using a patch-local register: CUT N..M @name then PUT @name <K inserts before original line K; PUT @name without <K inserts at the start. Neither takes payload.

Each body line must begin with + at column 1; everything after that marker is content, including indentation. + alone inserts a blank line; ++value inserts literal +value; +-item inserts literal -item. Do not use unified-diff deletion rows or include read-output prefixes. Use DEL for deletion (SWAP without body also deletes its target).

MCP tools/call params examples (anchors are illustrative: substitute anchors from your own read; JSON \n encodes an actual newline in the patch string):
Replace line: {"name":"patch","arguments":{"file":"a.txt","patch":"SWAP 2:ab:\n+    new value"}}
Replace range: {"name":"patch","arguments":{"file":"a.txt","patch":"SWAP 2:ab..4:\n+first\n+second"}}
Insert: {"name":"patch","arguments":{"file":"a.txt","patch":"INS.POST 2:ab:\n+added\n+\n++literal plus\n+-literal minus"}}
Delete: {"name":"patch","arguments":{"file":"a.txt","patch":"DEL 2:ab..4"}}
Multiple operations: {"name":"patch","arguments":{"file":"a.txt","patch":"SWAP 2:ab:\n+replacement\nDEL 5:cd\nINS.TAIL:\n+last","return_updated_anchors":true}}

All targets in a batch refer to the same pre-edit file, not intermediate output. Keep operations independent and non-overlapping. Unknown/malformed operation keywords can reject the whole patch; other parser warnings may accompany a successful edit. Parsing/application failures do not write the file; this is not a crash-safe or concurrent-write guarantee. On failure, inspect the error, read again for stale/invalid anchors, and rebuild rather than guessing or removing hashes. Success returns OK path#hash edits=N changed=N and rows (~N:hh|content modified, +N:hh|content inserted, -N deleted), plus parser warnings when present; full fresh anchors require return_updated_anchors (or server environment opt-in via HASHLINE_RETURN_ANCHORS). Writes are direct, not crash-safe transactions, and do not lock out concurrent writers."#;

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
                description: "Read a UTF-8 text file before editing. Returns [path#HASH] and N:hh|content for every visible line; copy N:hh anchors into patch headers.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Path to the existing UTF-8 text file to read"},
                        "json": {"type": "boolean", "description": "Return a JSON object with path, hash and lines instead of compact text; default false"}
                    },
                    "required": ["file"]
                })),
            },
            ToolDefinition {
                name: "patch".into(),
                description: "Edit an existing file using a textual patch and anchors from read. Supports replacement, insertion, deletion and block operations; see the patch parameter for grammar and MCP examples.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Existing UTF-8 text file to patch; use anchors from the latest read of this file"},
                        "patch": {"type": "string", "description": MCP_PATCH_FORMAT},
                        "dry_run": {"type": "boolean", "description": "Default false. Validate/apply in memory without writing. Parser warnings, when present, appear before a blank line and Dry-run result:; everything after that marker is the full proposed file text. Without warnings, the response starts with Dry-run result:. Not a diff; no fresh anchor listing, even if return_updated_anchors is true."},
                        "return_updated_anchors": {"type": "boolean", "description": "Default false. After a successful non-dry-run write, append the complete [path#HASH] and N:hh|content listing for follow-up edits. Otherwise only changed rows are returned, unless the server enables HASHLINE_RETURN_ANCHORS."}
                    },
                    "required": ["file", "patch"]
                })),
            },
            ToolDefinition {
                name: "write".into(),
                description:
                    "Create a text file, or replace its entire content with force=true. Prefer patch for edits to existing files. Returns OK path#hash lines=N by default.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Destination file path; existing files require force=true"},
                        "content": {"type": "string", "description": "Complete file content, not a patch; CRLF is normalized to LF and a leading BOM is stripped"},
                        "force": {"type": "boolean", "description": "Allow replacing an existing file; default false. This is a whole-file overwrite, not an anchored edit."},
                        "json": {"type": "boolean", "description": "Return a JSON object with path, hash and lines instead of compact text; default false"}
                    },
                    "required": ["file", "content"]
                })),
            },
            ToolDefinition {
                name: "find_block".into(),
                description: "Preview a likely enclosing block using brace, indentation or Ruby end heuristics. Returns numbered, hashed block lines; falls back to the anchor line if no block is found. Not tree-sitter analysis.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Existing UTF-8 text file whose block boundaries should be inspected"},
                        "anchor": {"type": "string", "description": "N:hh anchor copied from read near the desired block (1-based line number and hash)"}
                    },
                    "required": ["file", "anchor"]
                })),
            },
            ToolDefinition {
                name: "remove_file".into(),
                description: "Permanently delete an existing file, not individual lines. Returns OK path; no undo or anchor check.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "Path to the existing file to permanently delete"}
                    },
                    "required": ["file"]
                })),
            },
            ToolDefinition {
                name: "rename_file".into(),
                description: "Move an existing file to a destination that does not exist; creates destination parent directories. Returns OK src>dst; refuses to overwrite the destination.".into(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "src": {"type": "string", "description": "Path to the existing file to move"},
                        "dst": {"type": "string", "description": "New file path; must not already exist"}
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

    // Agent-native compact header + block lines
    let mut out = format!(
        "OK file={} lang={} lines={}\n",
        fc.path.display(),
        language,
        fc.len()
    );
    for i in start..=end {
        let entry = &entries[i];
        let h = hash::format_short_hash(entry.short_hash);
        out.push_str(&format!("{}:{}|{}\n", i + 1, h, entry.content));
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
                // Agent-native compact
                format!("OK {file}")
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
                // Agent-native compact
                format!("OK {src}>{dst}")
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_patch(
    file: &str,
    patch_str: &str,
    dry_run: bool,
    return_updated_anchors: bool,
) -> String {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return format!("Error: {e}"),
    };
    let (edits, warnings, _file_op, _aborted) = parse_patch(patch_str);

    // Surface warnings in the tool result so MCP clients see them
    // (Issue #93 — warnings are not just debug info for the terminal).
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
    let changeset = match crate::commands::patch::apply_edits_tracked(
        &mut lines,
        &entries,
        path,
        &edits,
        crate::commands::patch::count_operations(&edits),
    ) {
        Ok(cs) => cs,
        Err(e) => return format!("Error: {e}"),
    };

    let result = join_lines(&lines, fc.trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    let final_text = if line_ending == LineEnding::Crlf {
        restore_line_endings(&result, line_ending)
    } else {
        result
    };

    if dry_run {
        if warning_line.is_empty() {
            format!("Dry-run result:\n{final_text}")
        } else {
            format!(
                "{}\n\nDry-run result:\n{final_text}",
                warning_line.trim_start()
            )
        }
    } else {
        match crate::commands::common::fast_write(path, final_text.as_bytes()) {
            Ok(_) => {
                // Agent-native compact output: OK header + changed lines only
                let changed_count =
                    changeset.modified.len() + changeset.inserted.len() + changeset.deleted.len();
                let mut out = format!(
                    "OK {}#{} edits={} changed={}",
                    file, changeset.file_hash, changeset.edits_applied, changed_count,
                );
                for line in &changeset.modified {
                    out.push_str(&format!("\n~{}:{}|{}", line.line, line.hash, line.content));
                }
                for line in &changeset.inserted {
                    out.push_str(&format!("\n+{}:{}|{}", line.line, line.hash, line.content));
                }
                for &line_num in &changeset.deleted {
                    out.push_str(&format!("\n-{}", line_num));
                }
                if !warning_line.is_empty() {
                    out.push_str(&warning_line);
                }
                // Opt-in (issue #119): full fresh anchor listing in `read`
                // format so the caller can chain edits without re-reading.
                if return_updated_anchors || crate::commands::patch::emit_anchors_from_env() {
                    out.push_str(&format!("\n[{}#{}]", file, changeset.file_hash));
                    for line in &changeset.updated_anchors {
                        out.push_str(&format!("\n{}:{}|{}", line.line, line.hash, line.content));
                    }
                }
                out
            }
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

    // Compute hash and line count from the written content (canonical path).
    let file_hash = hash::compute_file_hash(&write_content);
    let line_count = write_content.lines().count();

    if json {
        // JSON: re-read for full line data (backward compatible)
        match FileContent::load(path) {
            Ok(fc) => {
                let entries = fc.lines_with_hashes();
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
            }
            Err(e) => format!("Written successfully.\nError re-reading file: {e}"),
        }
    } else {
        // Agent-native compact: status line only, no re-read
        format!("OK {}#{} lines={}", file, file_hash, line_count)
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

/// Build an MCP `CallToolResult` from a tool handler's text output.
///
/// Handlers report failures as text beginning with `Error: `. The MCP spec's
/// `isError` flag marks those results as failures, so a client — including
/// the CLI's daemon routing — can tell "the tool failed" from "the tool
/// succeeded and produced this text" without sniffing the payload.
fn tool_result(text: String) -> Value {
    if text.starts_with("Error: ") {
        serde_json::json!({"content": [{"type": "text", "text": text}], "isError": true})
    } else {
        serde_json::json!({"content": [{"type": "text", "text": text}]})
    }
}

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "read" | "hashline_read" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(tool_result(handle_read(file, json)))
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
            let return_updated_anchors = args
                .get("return_updated_anchors")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(tool_result(handle_patch(
                file,
                patch,
                dry_run,
                return_updated_anchors,
            )))
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
            Ok(tool_result(handle_write(file, content, force, json)))
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
            Ok(tool_result(handle_find_block(file, anchor)))
        }
        "remove_file" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .ok_or("missing 'file'")?;
            let json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(tool_result(handle_remove_file(file, json)))
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
            Ok(tool_result(handle_rename_file(src, dst, json)))
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

    // ---- Tool results must flag failures (bd-bx2) -------------------------
    //
    // The daemon answers with the MCP `CallToolResult` shape. A tool failure
    // has to be distinguishable from a success, otherwise a CLI command
    // routed through the daemon reports exit 0 for every error.

    #[test]
    fn tool_result_flags_failures_with_is_error() {
        let failed = tool_result("Error: file not found: missing.txt".to_string());
        assert_eq!(failed["isError"], json!(true));

        let ok = tool_result("OK file.txt#abcd lines=1".to_string());
        assert!(
            ok.get("isError").is_none(),
            "success must not be flagged: {ok}"
        );
    }

    #[test]
    fn tool_call_on_missing_file_sets_is_error() {
        let args = json!({"file": "/nonexistent/path/does/not/exist.txt"});
        let result = call_tool("read", &args).expect("call_tool should return a result");
        assert_eq!(result["isError"], json!(true), "{result}");
    }

    #[test]
    fn tool_call_success_has_no_is_error_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello\n").unwrap();
        let args = json!({"file": path.to_string_lossy()});
        let result = call_tool("read", &args).expect("call_tool should return a result");
        assert!(result.get("isError").is_none(), "{result}");
    }

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
        assert!(
            text.contains("hashline"),
            "instructions should mention hashline"
        );
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
    fn tools_list_preserves_api_and_describes_every_parameter() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/list".into(),
            params: None,
            id: Some(json!(1)),
        };
        let response = handle_request(&request, &mut Session::new());
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        let expected = [
            (
                "read",
                json!({"file": "string", "json": "boolean"}),
                json!(["file"]),
            ),
            (
                "patch",
                json!({"file": "string", "patch": "string", "dry_run": "boolean", "return_updated_anchors": "boolean"}),
                json!(["file", "patch"]),
            ),
            (
                "write",
                json!({"file": "string", "content": "string", "force": "boolean", "json": "boolean"}),
                json!(["file", "content"]),
            ),
            (
                "find_block",
                json!({"file": "string", "anchor": "string"}),
                json!(["file", "anchor"]),
            ),
            ("remove_file", json!({"file": "string"}), json!(["file"])),
            (
                "rename_file",
                json!({"src": "string", "dst": "string"}),
                json!(["src", "dst"]),
            ),
        ];
        assert_eq!(tools.len(), expected.len());
        for (name, parameter_types, required) in expected {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            assert!(
                !tool["description"].as_str().unwrap().trim().is_empty(),
                "{name}"
            );
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}");
            assert_eq!(schema["required"], required, "{name}");
            let properties = schema["properties"].as_object().unwrap();
            let types = parameter_types.as_object().unwrap();
            assert_eq!(properties.len(), types.len(), "{name}");
            for (parameter, expected_type) in types {
                let property = &properties[parameter];
                assert_eq!(&property["type"], expected_type, "{name}.{parameter}");
                assert!(
                    !property["description"].as_str().unwrap().trim().is_empty(),
                    "{name}.{parameter}"
                );
            }
        }
    }

    #[test]
    fn patch_batch_failures_leave_file_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let original = "first\nsecond\nthird\n";
        std::fs::write(&path, original).unwrap();
        let file = path.to_str().unwrap();
        let read: Value = serde_json::from_str(&handle_read(file, true)).unwrap();
        let first_hash = read["lines"][0]["hash"].as_str().unwrap();
        let second_hash = read["lines"][1]["hash"].as_str().unwrap();
        let stale_hash = if second_hash == "00" { "01" } else { "00" };
        let valid = format!("SWAP 1:{first_hash}:\n+replacement");
        for (patch, error_fragment) in [
            (
                format!("{valid}\nDEL 2:{stale_hash}"),
                "changed since last read",
            ),
            (
                format!("{valid}\nINS.PRE 2:{stale_hash}:\n+added"),
                "changed since last read",
            ),
            (
                format!("{valid}\nINS.POST 2:{stale_hash}:\n+added"),
                "changed since last read",
            ),
            (format!("{valid}\nSWAP"), "patch produced no edits"),
        ] {
            let request = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "tools/call".into(),
                params: Some(json!({"name": "patch", "arguments": {"file": file, "patch": patch}})),
                id: Some(json!(1)),
            };
            let response = handle_request(&request, &mut Session::new());
            assert!(response.error.is_none());
            let result = response.result.unwrap();
            assert_eq!(result["isError"], true, "{result}");
            assert!(
                result["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(error_fragment),
                "{result}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn advertised_patch_examples_apply_through_mcp() {
        let original = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let expected = [
            "one\n    new value\nthree\nfour\nfive\nsix\n",
            "one\nfirst\nsecond\nfive\nsix\n",
            "one\ntwo\nadded\n\n+literal plus\n-literal minus\nthree\nfour\nfive\nsix\n",
            "one\nfive\nsix\n",
            "one\nreplacement\nthree\nfour\nsix\nlast\n",
        ];
        let examples: Vec<Value> = MCP_PATCH_FORMAT
            .lines()
            .filter_map(|line| line.split_once(": {\"name\":").map(|(_, rest)| rest))
            .map(|rest| serde_json::from_str(&format!("{{\"name\":{rest}")).unwrap())
            .collect();
        assert_eq!(examples.len(), expected.len());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        for (example, expected_text) in examples.iter().zip(expected) {
            std::fs::write(&path, original).unwrap();
            let read: Value =
                serde_json::from_str(&handle_read(path.to_str().unwrap(), true)).unwrap();
            let mut args = example["arguments"].clone();
            args["file"] = json!(path.to_str().unwrap());
            // The examples deliberately use placeholders, not fabricated real hashes.
            args["patch"] = json!(
                args["patch"]
                    .as_str()
                    .unwrap()
                    .replace(
                        "2:ab",
                        &format!("2:{}", read["lines"][1]["hash"].as_str().unwrap())
                    )
                    .replace(
                        "5:cd",
                        &format!("5:{}", read["lines"][4]["hash"].as_str().unwrap())
                    )
            );
            let result = call_tool(example["name"].as_str().unwrap(), &args).unwrap();
            assert!(result.get("isError").is_none(), "{result}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected_text);
        }
    }

    #[test]
    fn patch_preview_preserves_parser_warnings_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let file = path.to_str().unwrap();
        for original in ["alpha\nbeta\n", "alpha\r\nbeta\r\n"] {
            std::fs::write(&path, original).unwrap();
            let original_bytes = std::fs::read(&path).unwrap();
            let read: Value = serde_json::from_str(&handle_read(file, true)).unwrap();
            let anchor = format!("2:{}", read["lines"][1]["hash"].as_str().unwrap());
            let patch = format!("SWAP {anchor}:\n-beta\n+added");
            let preview = call_tool(
                "patch",
                &json!({"file": file, "patch": patch, "dry_run": true}),
            )
            .unwrap();
            assert!(preview.get("isError").is_none(), "{preview}");
            let preview_text = preview["content"][0]["text"].as_str().unwrap();
            let (warnings, proposed) = preview_text.split_once("\n\nDry-run result:\n").unwrap();
            assert!(warnings.starts_with("warnings:\n"), "{warnings}");
            assert!(
                warnings.contains(crate::messages::MINUS_ROW_REJECTED),
                "{warnings}"
            );
            let expected = if original.contains("\r\n") {
                "alpha\r\n-beta\r\nadded\r\n"
            } else {
                "alpha\n-beta\nadded\n"
            };
            assert_eq!(proposed, expected);
            assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

            let applied = call_tool(
                "patch",
                &json!({"file": file, "patch": patch, "dry_run": false}),
            )
            .unwrap();
            assert!(applied.get("isError").is_none(), "{applied}");
            let applied_text = applied["content"][0]["text"].as_str().unwrap();
            assert!(applied_text.starts_with("OK "), "{applied_text}");
            assert!(applied_text.contains(warnings), "{applied_text}");
            assert_eq!(std::fs::read(&path).unwrap(), proposed.as_bytes());
        }
    }

    #[test]
    fn patch_preview_and_anchor_opt_in_match_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "old\n").unwrap();
        let file = path.to_str().unwrap();
        let patch = "INS.TAIL:\n+new";
        let preview = handle_patch(file, patch, true, true);
        assert_eq!(preview, "Dry-run result:\nold\nnew\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\n");

        let result = handle_patch(file, patch, false, false);
        assert!(result.starts_with("OK "), "{result}");
        if !crate::commands::patch::emit_anchors_from_env() {
            assert!(!result.contains(&format!("\n[{file}#")), "{result}");
        }
        let result = handle_patch(file, "INS.HEAD:\n+first", false, true);
        let fresh_read = handle_read(file, false);
        // read currently includes an extra empty segment for a trailing newline.
        let fresh_rows = fresh_read
            .lines()
            .skip(1)
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(result.contains(&format!("\n[{file}#")), "{result}");
        assert!(result.ends_with(&fresh_rows), "{result}");
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
