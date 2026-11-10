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
use crate::normalize::{detect_line_ending, restore_line_endings};
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

#[derive(Clone, Debug, Serialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// `None` = notification (no `id` member — no response). `Some(Value)`
    /// = request to answer, including `Some(Value::Null)` for an explicit
    /// `"id": null`, which JSON-RPC requires be echoed back as `"id": null`.
    #[serde(default)]
    pub id: Option<Value>,
}

impl<'de> Deserialize<'de> for JsonRpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("JSON-RPC request must be an object"))?;
        let method = obj
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("missing 'method' member"))?;
        // `Option<Value>` field deserialization cannot distinguish an absent
        // `id` (notification) from `"id": null` (a request that must be
        // answered) — both collapse to `None`, which silently dropped every
        // null-id request. Inspect the object directly instead.
        let id = obj.get("id").cloned();
        Ok(JsonRpcRequest {
            jsonrpc: obj
                .get("jsonrpc")
                .and_then(Value::as_str)
                .unwrap_or("2.0")
                .to_string(),
            method: method.to_string(),
            params: obj.get("params").cloned(),
            id,
        })
    }
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
                        "dry_run": {"type": "boolean", "description": "Default false. Validate/apply in memory without writing. Returns a bounded unified-diff-style change list (same @@/+/− rows as `patch --dry-run`), never the whole file. Parser and apply-time warnings, when present, appear before the diff. Not a diff of context; no fresh anchor listing, even if return_updated_anchors is true."},
                        "return_updated_anchors": {"type": "boolean", "description": "Default false. After a successful non-dry-run write, append the complete [path#HASH] and N:hh|content listing for follow-up edits. Otherwise only changed rows are returned, unless the server enables HASHLINE_RETURN_ANCHORS."},
                        "json": {"type": "boolean", "description": "Return the result as a JSON object (success, file, hash, edits_applied, changed, warnings, updated_anchors when requested) instead of compact text; default false. structuredContent carries the same object either way."}
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
                        "anchor": {"type": "string", "description": "N:hh anchor copied from read near the desired block (1-based line number and hash)"},
                        "json": {"type": "boolean", "description": "Return the block payload as a JSON object (file, line_count, language, block_lines) instead of compact text; default false. structuredContent carries the same object either way."}
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
                        "file": {"type": "string", "description": "Path to the existing file to permanently delete"},
                        "json": {"type": "boolean", "description": "Return a JSON object instead of compact text; default false"}
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
                        "dst": {"type": "string", "description": "New file path; must not already exist"},
                        "json": {"type": "boolean", "description": "Return a JSON object instead of compact text; default false"}
                    },
                    "required": ["src", "dst"]
                })),
            },
        ],
    }
}

/// A tool handler's result: the text payload for `content[0].text` plus an
/// optional structured object for `structuredContent` (which also backs
/// `json: true` text output, so both channels always agree). `None` omits
/// `structuredContent` entirely — used by compact `read`, where materializing
/// the per-line JSON payload cost ~1GB of peak RSS on large files for data
/// the caller never asked for.
struct ToolOutput {
    text: String,
    structured: Option<Value>,
}

fn ok_output(text: String, structured: Value) -> ToolOutput {
    ToolOutput {
        text,
        structured: Some(structured),
    }
}

/// Success output with no `structuredContent` member at all.
fn ok_output_text_only(text: String) -> ToolOutput {
    ToolOutput {
        text,
        structured: None,
    }
}

/// Convert a [`HashlineError`] into a failed tool result. The text keeps the
/// `Error: ` prefix (the daemon/CLI error heuristic keys on it) and adds
/// `kind:`/`hint:` lines; the structured payload carries the same fields —
/// plus the variant's structured `details` — so MCP clients get the error
/// kind and recovery hint instead of a bare prose string.
fn err_output(error: &HashlineError) -> ToolOutput {
    let mut text = format!("Error: {error}");
    text.push_str(&format!("\nkind: {}", error.kind()));
    if let Some(hint) = error.hint() {
        text.push_str(&format!("\nhint: {hint}"));
    }
    if let Some(url) = error.report_url() {
        text.push_str(&format!("\nreport: {url}"));
    }
    let mut structured = serde_json::json!({
        "success": false,
        "kind": error.kind(),
        "error": error.to_string(),
    });
    if let Some(hint) = error.hint() {
        structured["hint"] = Value::String(hint.to_string());
    }
    if let Some(command) = error.command() {
        structured["command"] = Value::String(command.to_string());
    }
    if let Some(details) = crate::output::error_details(error) {
        structured["details"] = details;
    }
    if let Some(url) = error.report_url() {
        structured["report_url"] = Value::String(url.to_string());
    }
    ToolOutput {
        text,
        structured: Some(structured),
    }
}

fn io_err(context: &str, error: std::io::Error) -> ToolOutput {
    err_output(&HashlineError::Io(std::io::Error::new(
        error.kind(),
        format!("{context}: {error}"),
    )))
}

/// Line entries visible to callers. `lines_with_hashes` includes the
/// trailing empty segment `split('\n')` produces when the file ends with a
/// newline — that segment is not a real line, and emitting it produced a
/// phantom `N:hh|` anchor that `patch` then rejected as out of bounds.
fn visible_entries(fc: &FileContent) -> Vec<crate::document::LineEntry> {
    let mut entries = fc.lines_with_hashes();
    if fc.trailing_newline && entries.last().is_some_and(|e| e.content.is_empty()) {
        entries.pop();
    }
    entries
}

/// JSON line entries in the canonical `read --json` shape ({n, hash,
/// content}) — shared by read/write/find_block results.
fn lines_json(entries: &[crate::document::LineEntry]) -> Vec<Value> {
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            serde_json::json!({
                "n": i + 1,
                "hash": hash::format_short_hash(entry.short_hash),
                "content": entry.content,
            })
        })
        .collect()
}

fn handle_read(file: &str, json: bool) -> ToolOutput {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return err_output(&e),
    };
    let entries = visible_entries(&fc);
    if json {
        let structured = serde_json::json!({
            "path": fc.path.display().to_string(),
            "hash": fc.hash,
            "lines": lines_json(&entries),
        });
        return ok_output(
            serde_json::to_string(&structured).unwrap_or_default(),
            structured,
        );
    }
    // Agent-native compact: [path#hash] header + N:hh|content rows.
    // The path is escaped so `#`/newline in filenames cannot corrupt the
    // anchor header. No structuredContent: building the per-line JSON
    // payload here made MCP `read` ~7.5x slower and ~1GB heavier on large
    // files even though compact callers only consume the text.
    let mut out = format!(
        "[{}#{}]\n",
        crate::output::escape_compact_field(&fc.path.display().to_string()),
        fc.hash
    );
    for (i, entry) in entries.iter().enumerate() {
        out.push_str(&format!(
            "{}:{}|{}\n",
            i + 1,
            hash::format_short_hash(entry.short_hash),
            entry.content
        ));
    }
    ok_output_text_only(out)
}

fn handle_find_block(file: &str, anchor_str: &str, json: bool) -> ToolOutput {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return err_output(&e),
    };
    // Resolve through the shared CLI implementation so MCP and
    // `hashline find-block` agree on boundaries *and* on failure: the old
    // MCP-only reimplementation silently fell back to the anchor line where
    // the CLI reports BLOCK_UNRESOLVED.
    let entries = fc.lines_with_hashes();
    let mut payload =
        match crate::commands::find_block::find_block_payload(&fc, &entries, anchor_str) {
            Ok(p) => p,
            Err(e) => return err_output(&e),
        };
    // `lines_with_hashes` includes the phantom trailing empty segment for
    // newline-terminated files; it is not a real line and its anchor is
    // rejected by `patch`. Drop it from the emitted block (the CLI
    // find-block has the same defect — tracked separately).
    payload
        .block_lines
        .retain(|l| !(l.content.is_empty() && l.n > fc.len()));
    let structured = serde_json::to_value(&payload).unwrap_or_default();
    if json {
        return ok_output(
            serde_json::to_string(&structured).unwrap_or_default(),
            structured,
        );
    }
    let lang = payload.language.as_deref().unwrap_or("Unknown");
    let mut out = format!(
        "OK file={} lang={} lines={}\n",
        crate::output::escape_compact_field(&payload.file),
        lang,
        payload.line_count
    );
    for line in &payload.block_lines {
        out.push_str(&format!("{}:{}|{}\n", line.n, line.hash, line.content));
    }
    ok_output(out, structured)
}

fn handle_remove_file(file: &str, json: bool) -> ToolOutput {
    let path = std::path::Path::new(file);
    // Same guards as CLI `remove` (bd-mcp-serve-parity-batch-nh8w):
    // device/reserved paths, existence, directory rejection.
    if let Err(e) = crate::commands::common::validate_remove_path(path) {
        return err_output(&e);
    }
    match std::fs::remove_file(path) {
        Ok(_) => {
            let structured = serde_json::json!({"success": true, "file": file});
            if json {
                ok_output(
                    serde_json::to_string(&structured).unwrap_or_default(),
                    structured,
                )
            } else {
                // Agent-native compact
                ok_output(
                    format!("OK {}", crate::output::escape_compact_field(file)),
                    structured,
                )
            }
        }
        Err(e) => io_err("failed to remove file", e),
    }
}

fn handle_rename_file(src: &str, dst: &str, json: bool) -> ToolOutput {
    let src_path = std::path::Path::new(src);
    let dst_path = std::path::Path::new(dst);
    // Same guards as CLI `rename` without --force (bd-mcp-serve-parity-
    // batch-nh8w): device/reserved paths, source must be a regular file
    // (directories rejected — fs::rename would move whole trees on Unix),
    // src/dst must not alias, destination must not exist.
    if let Err(e) = crate::commands::common::validate_rename_paths(src_path, dst_path, false) {
        return err_output(&e);
    }
    if let Some(parent) = dst_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return io_err("failed to create destination parent directories", e);
        }
    }
    match std::fs::rename(src_path, dst_path) {
        Ok(_) => {
            let structured = serde_json::json!({"success": true, "src": src, "dst": dst});
            if json {
                ok_output(
                    serde_json::to_string(&structured).unwrap_or_default(),
                    structured,
                )
            } else {
                // Agent-native compact
                ok_output(
                    format!(
                        "OK {}>{}",
                        crate::output::escape_compact_field(src),
                        crate::output::escape_compact_field(dst)
                    ),
                    structured,
                )
            }
        }
        Err(e) => io_err("failed to rename file", e),
    }
}

/// Build the `changed` array shared by compact-adjacent JSON output and
/// `structuredContent` — same shape as `patch --json`.
fn changed_json(changeset: &crate::types::ChangeSet) -> Value {
    let mut changed: Vec<Value> = Vec::new();
    for l in &changeset.modified {
        changed.push(serde_json::json!({
            "type": "modified", "line": l.line, "hash": l.hash, "content": l.content,
        }));
    }
    for l in &changeset.inserted {
        changed.push(serde_json::json!({
            "type": "inserted", "line": l.line, "hash": l.hash, "content": l.content,
        }));
    }
    for &n in &changeset.deleted {
        changed.push(serde_json::json!({"type": "deleted", "line": n}));
    }
    Value::Array(changed)
}

/// `updated_anchors` entries in the canonical `read --json` line shape
/// ({n, hash, content}) — identical to `patch --json --emit-anchors`.
fn updated_anchors_json(changeset: &crate::types::ChangeSet) -> Value {
    Value::Array(
        changeset
            .updated_anchors
            .iter()
            .map(|l| {
                serde_json::json!({
                    "n": l.line,
                    "hash": l.hash,
                    "content": l.content,
                })
            })
            .collect(),
    )
}

fn handle_patch(
    file: &str,
    patch_str: &str,
    dry_run: bool,
    return_updated_anchors: bool,
    json: bool,
) -> ToolOutput {
    let path = Path::new(file);
    let fc = match FileContent::load(path) {
        Ok(fc) => fc,
        Err(e) => return err_output(&e),
    };
    let (edits, warnings, file_op, aborted) = parse_patch(patch_str);

    // File-level ops (REM/MV) parse successfully but have no effect here —
    // reporting OK would claim an edit that never happened. The dedicated
    // remove_file/rename_file tools cover them.
    if file_op.is_some() {
        return err_output(&HashlineError::PatchRejected {
            reason: "patch contains a file-level operation (REM/MV) which the patch tool does not \
                     support; use remove_file or rename_file instead"
                .into(),
        });
    }
    if aborted {
        // Mirror the CLI: a rejected patch is a failure, not an empty edit.
        let reason = warnings
            .first()
            .map(|w| w.lines().next().unwrap_or(w).to_string())
            .unwrap_or_else(|| "the patch was rejected before any edits were applied".to_string());
        return err_output(&HashlineError::PatchRejected {
            reason: reason.into_boxed_str(),
        });
    }
    if edits.is_empty() {
        let error = if warnings.is_empty() {
            HashlineError::EmptyPatch
        } else {
            HashlineError::EmptyPatchWithReason {
                reason: warnings[0].clone().into_boxed_str(),
            }
        };
        return err_output(&error);
    }

    let (mut lines, _) = split_text(&fc.normalized);
    let orig_line_count = lines.len();

    let entries = fc.lines_with_hashes();
    // Drain stale apply warnings from any earlier failed apply, then collect
    // the ones this apply emits (boundary-echo repairs, delimiter warnings).
    // They change file content and must reach the client — stderr is
    // invisible to MCP clients.
    let _ = crate::commands::patch::take_apply_warnings();
    // Anchor listing is opt-in and never emitted for dry-run, so don't pay
    // for it in the changeset there either.
    let want_anchors =
        (return_updated_anchors || crate::commands::patch::emit_anchors_from_env()) && !dry_run;
    let changeset = match crate::commands::patch::apply_edits_tracked(
        &mut lines,
        &entries,
        path,
        &edits,
        crate::commands::patch::count_operations(&edits),
        fc.trailing_newline,
        want_anchors,
    ) {
        Ok(cs) => cs,
        Err(e) => return err_output(&e),
    };
    // Mixed-EOL files are canonicalized to the dominant ending on
    // write-back — surface it in-band since MCP clients cannot see the
    // CLI's stderr warning (bd-write-safety-batch-sz62).
    let mixed_eol_warning = crate::normalize::has_mixed_line_endings(&fc.raw).then(|| {
        format!(
            "file has mixed line endings; write-back uses the dominant {:?} ending for every line",
            detect_line_ending(&fc.raw)
        )
    });
    let mut all_warnings = warnings.clone();
    if let Some(w) = mixed_eol_warning {
        all_warnings.push(w);
    }
    for w in crate::commands::patch::take_apply_warnings() {
        if !all_warnings.contains(&w) {
            all_warnings.push(w);
        }
    }

    let trailing_newline = crate::commands::patch::effective_trailing_newline(
        &edits,
        orig_line_count,
        fc.trailing_newline,
        !lines.is_empty(),
    );
    let result = join_lines(&lines, trailing_newline);
    let line_ending = detect_line_ending(&fc.raw);
    // Restore the file's dominant line ending (LF/CRLF/CR) and re-attach the
    // BOM `FileContent::load` stripped — same write-back contract as the
    // CLI `patch` command. `result` (LF, BOM-free) is kept for the dry-run
    // diff; `final_text` is what lands on disk.
    let mut final_text = restore_line_endings(&result, line_ending);
    if !fc.bom.is_empty() {
        final_text.insert_str(0, &fc.bom);
    }

    let emit_anchors = return_updated_anchors || crate::commands::patch::emit_anchors_from_env();

    if dry_run {
        // Bounded output: a unified-diff-style change list (same rows as
        // `patch --dry-run`), never the entire proposed file — dumping the
        // file cost ~33KB per call where the CLI diff is ~62B.
        // Diff the LF-normalized, BOM-free `result` — `final_text` carries
        // the file's EOL style and BOM which would diff as spurious lines.
        let diff_text = &result;
        let diff_lines = crate::commands::patch::format_diff(&fc.normalized, diff_text);
        let structured = serde_json::json!({
            "success": true,
            "dry_run": true,
            "file": file,
            "hash": changeset.file_hash,
            "edits_applied": changeset.edits_applied,
            "changed": changed_json(&changeset),
            "diff": diff_lines,
            "warnings": all_warnings,
        });
        if json {
            return ok_output(
                serde_json::to_string(&structured).unwrap_or_default(),
                structured,
            );
        }
        let mut out = String::new();
        if !all_warnings.is_empty() {
            out.push_str("warnings:\n");
            for w in &all_warnings {
                out.push_str(&format!("  - {w}\n"));
            }
        }
        for dl in &diff_lines {
            out.push_str(dl);
            out.push('\n');
        }
        return ok_output(out, structured);
    }

    // Same write-time stale-file guard + post-write byte verification as
    // the CLI patch path.
    match crate::commands::patch::write_patch_output(path, &final_text, &fc, false) {
        Ok(()) => {
            let changed_count =
                changeset.modified.len() + changeset.inserted.len() + changeset.deleted.len();
            let mut structured = serde_json::json!({
                "success": true,
                "file": file,
                "hash": changeset.file_hash,
                "edits_applied": changeset.edits_applied,
                "changed": changed_json(&changeset),
                "warnings": all_warnings,
            });
            if emit_anchors {
                structured["updated_anchors"] = updated_anchors_json(&changeset);
            }
            if json {
                return ok_output(
                    serde_json::to_string(&structured).unwrap_or_default(),
                    structured,
                );
            }
            // Agent-native compact output: OK header + changed lines only.
            let mut out = format!(
                "OK {}#{} edits={} changed={}",
                crate::output::escape_compact_field(file),
                changeset.file_hash,
                changeset.edits_applied,
                changed_count,
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
            if !all_warnings.is_empty() {
                out.push_str("\nwarnings:");
                for w in &all_warnings {
                    out.push_str(&format!("\n  - {w}"));
                }
            }
            // Opt-in (issue #119): full fresh anchor listing in `read`
            // format so the caller can chain edits without re-reading.
            if emit_anchors {
                out.push_str(&format!(
                    "\n[{}#{}]",
                    crate::output::escape_compact_field(file),
                    changeset.file_hash
                ));
                for line in &changeset.updated_anchors {
                    out.push_str(&format!("\n{}:{}|{}", line.line, line.hash, line.content));
                }
            }
            ok_output(out, structured)
        }
        Err(e) => err_output(&e),
    }
}

fn handle_write(file: &str, content: &str, force: bool, json: bool) -> ToolOutput {
    let path = Path::new(file);

    // Check file exists — refuse unless force=true
    if path.exists() && !force {
        return err_output(&HashlineError::TargetExists {
            path: file.to_string(),
        });
    }

    let normalized = crate::normalize::normalize_to_lf(content);
    let bom_result = crate::normalize::strip_bom(&normalized);
    let write_content = bom_result.text;

    // Overwriting an existing file preserves its BOM marker and dominant
    // line-ending style (same rule as `hashline write --force`).
    let (bom, line_ending) = crate::document::probe_existing_style(path);
    let mut disk_content = crate::normalize::restore_line_endings(&write_content, line_ending);
    if !bom.is_empty() {
        disk_content.insert_str(0, &bom);
    }

    if let Err(e) = crate::commands::common::fast_write(path, disk_content.as_bytes()) {
        return err_output(&e);
    }
    // Byte-exact post-write check: a concurrent overwrite is reported as
    // CONCURRENT_MODIFICATION, never a silent OK.
    if let Err(e) = crate::commands::common::verify_written_bytes(path, disk_content.as_bytes()) {
        return err_output(&e);
    }

    // Compute hash and line count from the written content (canonical path).
    let file_hash = hash::compute_file_hash(&write_content);
    let line_count = write_content.lines().count();

    if json {
        // Re-read for the full line listing. A write that cannot be verified
        // is a *partial* failure — report it as an error, not "Written
        // successfully" text that the isError heuristic misses.
        match FileContent::load(path) {
            Ok(fc) => {
                let entries = visible_entries(&fc);
                let structured = serde_json::json!({
                    "success": true,
                    "path": fc.path.display().to_string(),
                    "hash": fc.hash,
                    "lines": lines_json(&entries),
                });
                ok_output(
                    serde_json::to_string(&structured).unwrap_or_default(),
                    structured,
                )
            }
            Err(e) => err_output(&HashlineError::Io(std::io::Error::other(format!(
                "file written but re-read for verification failed: {e}"
            )))),
        }
    } else {
        // Agent-native compact: status line only, no re-read
        ok_output(
            format!(
                "OK {}#{} lines={}",
                crate::output::escape_compact_field(file),
                file_hash,
                line_count
            ),
            serde_json::json!({
                "success": true,
                "file": file,
                "hash": file_hash,
                "lines": line_count,
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

/// Build an MCP `CallToolResult` from a tool handler's output.
///
/// Handlers report failures either as text beginning with `Error: ` or as a
/// structured payload with `"success": false`. The MCP spec's `isError`
/// flag marks those results as failures, so a client — including the CLI's
/// daemon routing — can tell "the tool failed" from "the tool succeeded and
/// produced this text" without sniffing the payload. `structuredContent`
/// carries the handler's structured object on both success and failure.
fn tool_result(output: ToolOutput) -> Value {
    let is_error = output.text.starts_with("Error: ")
        || output
            .structured
            .as_ref()
            .and_then(|s| s.get("success"))
            .and_then(Value::as_bool)
            == Some(false);
    let mut result = serde_json::json!({
        "content": [{"type": "text", "text": output.text}],
    });
    if let Some(structured) = output.structured {
        result["structuredContent"] = structured;
    }
    if is_error {
        result["isError"] = serde_json::json!(true);
    }
    result
}

/// Extract a required string parameter, or fail with JSON-RPC `-32602`
/// (Invalid params) — parameter problems are not "method not found".
fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, JsonRpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| JsonRpcError {
            code: -32602,
            message: format!("missing or invalid '{key}' parameter"),
            data: None,
        })
}

fn opt_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn call_tool(name: &str, args: &Value) -> Result<Value, JsonRpcError> {
    let output = match name {
        "read" | "hashline_read" => handle_read(require_str(args, "file")?, opt_bool(args, "json")),
        "patch" | "hashline_patch" => handle_patch(
            require_str(args, "file")?,
            require_str(args, "patch")?,
            opt_bool(args, "dry_run"),
            opt_bool(args, "return_updated_anchors"),
            opt_bool(args, "json"),
        ),
        "write" | "hashline_write" => handle_write(
            require_str(args, "file")?,
            require_str(args, "content")?,
            opt_bool(args, "force"),
            opt_bool(args, "json"),
        ),
        "find_block" | "hashline_find_block" => handle_find_block(
            require_str(args, "file")?,
            require_str(args, "anchor")?,
            opt_bool(args, "json"),
        ),
        "remove_file" => handle_remove_file(require_str(args, "file")?, opt_bool(args, "json")),
        "rename_file" => handle_rename_file(
            require_str(args, "src")?,
            require_str(args, "dst")?,
            opt_bool(args, "json"),
        ),
        _ => {
            return Err(JsonRpcError {
                code: -32602,
                message: format!("unknown tool: {name}"),
                data: None,
            });
        }
    };
    Ok(tool_result(output))
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
    let result: Result<Value, JsonRpcError> = match request.method.as_str() {
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
            let tool_name = params.get("name").and_then(|v| v.as_str());
            match tool_name {
                Some(name) => {
                    let arguments = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    call_tool(name, &arguments)
                }
                // A missing tool name is an invalid-params error (-32602),
                // not "method not found" (-32601).
                None => Err(JsonRpcError {
                    code: -32602,
                    message: "missing 'name' parameter for tools/call".to_string(),
                    data: None,
                }),
            }
        }
        "ping" => Ok(serde_json::json!({})),
        // -32601 Method not found — reserved for unknown JSON-RPC methods.
        _ => Err(JsonRpcError {
            code: -32601,
            message: format!("unknown method: {}", request.method),
            data: None,
        }),
    };

    match result {
        Ok(data) => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(data),
            error: None,
        },
        Err(error) => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        },
    }
}

pub fn write_error<W: Write>(
    writer: &mut W,
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> io::Result<()> {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        // JSON-RPC requires `"id": null` on errors where the request id
        // could not be determined (e.g. parse errors) — omitting the member
        // is a spec violation that strict clients reject.
        id: Some(id.unwrap_or(Value::Null)),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data,
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
    // Propagate the JSON-RPC error code: -32602 for parameter problems,
    // never the -32601 that was previously applied to everything.
    call_tool(tool_name, &args)
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
                // The writer is buffered — without a flush the parse-error
                // response never reaches the client and the request hangs.
                writer.flush().map_err(HashlineError::Io)?;
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
        let failed = tool_result(err_output(&HashlineError::FileNotFound {
            path: "missing.txt".into(),
        }));
        assert_eq!(failed["isError"], json!(true), "{failed}");
        // The structured error must carry kind/hint, not just prose.
        let sc = &failed["structuredContent"];
        assert_eq!(sc["success"], json!(false), "{sc}");
        assert_eq!(sc["kind"], json!("FILE_NOT_FOUND"), "{sc}");
        assert!(sc["hint"].is_string(), "{sc}");
        assert!(sc["details"]["path"].is_string(), "{sc}");

        let ok = tool_result(ok_output(
            "OK file.txt#abcd lines=1".to_string(),
            json!({"success": true}),
        ));
        assert!(
            ok.get("isError").is_none(),
            "success must not be flagged: {ok}"
        );
        assert!(ok["structuredContent"].is_object(), "{ok}");
    }

    #[test]
    fn tool_result_flags_structured_failure_without_error_prefix() {
        // A handler that reports failure only via `"success": false` (e.g. a
        // JSON-mode error payload) must still set isError.
        let result = tool_result(ok_output(
            "{\"success\":false,\"kind\":\"IO\"}".to_string(),
            json!({"success": false, "kind": "IO"}),
        ));
        assert_eq!(result["isError"], json!(true), "{result}");
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
                json!({"file": "string", "patch": "string", "dry_run": "boolean", "return_updated_anchors": "boolean", "json": "boolean"}),
                json!(["file", "patch"]),
            ),
            (
                "write",
                json!({"file": "string", "content": "string", "force": "boolean", "json": "boolean"}),
                json!(["file", "content"]),
            ),
            (
                "find_block",
                json!({"file": "string", "anchor": "string", "json": "boolean"}),
                json!(["file", "anchor"]),
            ),
            (
                "remove_file",
                json!({"file": "string", "json": "boolean"}),
                json!(["file"]),
            ),
            (
                "rename_file",
                json!({"src": "string", "dst": "string", "json": "boolean"}),
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
        let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
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
            (format!("{valid}\nSWAP"), "malformed operation"),
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
                serde_json::from_str(&handle_read(path.to_str().unwrap(), true).text).unwrap();
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
            let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
            let anchor = format!("2:{}", read["lines"][1]["hash"].as_str().unwrap());
            let patch = format!("SWAP {anchor}:\n-beta\n+added");
            let preview = call_tool(
                "patch",
                &json!({"file": file, "patch": patch, "dry_run": true}),
            )
            .unwrap();
            assert!(preview.get("isError").is_none(), "{preview}");
            let preview_text = preview["content"][0]["text"].as_str().unwrap();
            // Bounded diff output: parser warnings first, then diff rows —
            // never the entire proposed file.
            assert!(preview_text.starts_with("warnings:\n"), "{preview_text}");
            assert!(
                preview_text.contains(crate::messages::MINUS_ROW_REJECTED),
                "{preview_text}"
            );
            assert!(preview_text.contains("@@ -- ++ @@"), "{preview_text}");
            let expected = if original.contains("\r\n") {
                "alpha\r\n-beta\r\nadded\r\n"
            } else {
                "alpha\n-beta\nadded\n"
            };
            assert_ne!(preview_text.trim(), expected, "{preview_text}");
            assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

            let applied = call_tool(
                "patch",
                &json!({"file": file, "patch": patch, "dry_run": false}),
            )
            .unwrap();
            assert!(applied.get("isError").is_none(), "{applied}");
            let applied_text = applied["content"][0]["text"].as_str().unwrap();
            assert!(applied_text.starts_with("OK "), "{applied_text}");
            assert!(
                applied_text.contains(crate::messages::MINUS_ROW_REJECTED),
                "{applied_text}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
        }
    }

    #[test]
    fn patch_preview_and_anchor_opt_in_match_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "old\n").unwrap();
        let file = path.to_str().unwrap();
        let patch = "INS.TAIL:\n+new";
        let preview = handle_patch(file, patch, true, true, false);
        // Bounded unified-diff output, not the whole proposed file.
        assert_eq!(preview.text, "@@ -- ++ @@\n+new\n", "{}", preview.text);
        assert_eq!(preview.structured.as_ref().unwrap()["dry_run"], json!(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\n");

        let result = handle_patch(file, patch, false, false, false);
        assert!(result.text.starts_with("OK "), "{}", result.text);
        if !crate::commands::patch::emit_anchors_from_env() {
            assert!(
                !result.text.contains(&format!("\n[{file}#")),
                "{}",
                result.text
            );
        }
        let result = handle_patch(file, "INS.HEAD:\n+first", false, true, false);
        let fresh_read = handle_read(file, false).text;
        let fresh_rows = fresh_read
            .lines()
            .skip(1)
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            result.text.contains(&format!("\n[{file}#")),
            "{}",
            result.text
        );
        assert!(result.text.ends_with(&fresh_rows), "{}", result.text);
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

    // ---- bd-mcp-surface-2tv regressions ----------------------------------

    #[test]
    fn json_rpc_null_id_is_answered_with_null() {
        // `"id": null` is a request, not a notification — it must be
        // answered with `"id": null` echoed back.
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"ping","id":null}"#).unwrap();
        assert_eq!(req.id, Some(Value::Null));
        let resp = handle_request(&req, &mut Session::new());
        assert!(resp.error.is_none(), "{resp:?}");
        assert_eq!(resp.id, Some(Value::Null));
        let wire = serde_json::to_string(&resp).unwrap();
        assert!(wire.contains("\"id\":null"), "{wire}");

        // An absent `id` member is a notification: no response.
        let notif: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"ping"}"#).unwrap();
        assert!(notif.id.is_none());
    }

    #[test]
    fn json_rpc_error_codes_distinguish_params_from_method() {
        let mut session = Session::new();
        // Missing required tool argument → -32602 Invalid params.
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/call".into(),
            params: Some(json!({"name": "read", "arguments": {}})),
            id: Some(json!(7)),
        };
        let resp = handle_request(&req, &mut session);
        assert_eq!(resp.error.as_ref().unwrap().code, -32602, "{resp:?}");
        assert_eq!(resp.id, Some(json!(7)));

        // Unknown tool name → -32602 (it is a params problem).
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/call".into(),
            params: Some(json!({"name": "nope", "arguments": {}})),
            id: Some(json!(8)),
        };
        let resp = handle_request(&req, &mut session);
        assert_eq!(resp.error.as_ref().unwrap().code, -32602, "{resp:?}");

        // Unknown JSON-RPC method → -32601 Method not found.
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "bogus/method".into(),
            params: None,
            id: Some(json!(9)),
        };
        let resp = handle_request(&req, &mut session);
        assert_eq!(resp.error.as_ref().unwrap().code, -32601, "{resp:?}");
    }

    #[test]
    fn read_omits_phantom_trailing_empty_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let file = path.to_str().unwrap();

        let compact = handle_read(file, false).text;
        let rows: Vec<&str> = compact.lines().skip(1).collect();
        assert_eq!(rows.len(), 2, "{compact}");

        let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
        assert_eq!(read["lines"].as_array().unwrap().len(), 2, "{read}");
    }

    #[test]
    fn find_block_uses_cli_semantics_and_errors_like_cli() {
        let dir = tempfile::tempdir().unwrap();
        // A .rs file with no braces: the CLI reports BLOCK_UNRESOLVED; the
        // old MCP reimplementation silently fell back to the anchor line.
        let path = dir.path().join("plain.rs");
        std::fs::write(&path, "let x = 1;\nlet y = 2;\n").unwrap();
        let file = path.to_str().unwrap();
        let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
        let anchor = format!("1:{}", read["lines"][0]["hash"].as_str().unwrap());
        let result = call_tool("find_block", &json!({"file": file, "anchor": anchor})).unwrap();
        assert_eq!(result["isError"], json!(true), "{result}");
        assert_eq!(
            result["structuredContent"]["kind"],
            json!("BLOCK_UNRESOLVED"),
            "{result}"
        );

        // Positive case: an indent-structured Python block resolves and does
        // not emit the phantom trailing empty line.
        let py = dir.path().join("m.py");
        std::fs::write(&py, "def f():\n    x = 1\n    return x\n\ny = 2\n").unwrap();
        let py_file = py.to_str().unwrap();
        let read: Value = serde_json::from_str(&handle_read(py_file, true).text).unwrap();
        let anchor = format!("2:{}", read["lines"][1]["hash"].as_str().unwrap());
        let result = call_tool("find_block", &json!({"file": py_file, "anchor": anchor})).unwrap();
        assert!(result.get("isError").is_none(), "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("OK file="), "{text}");
        assert!(text.contains("lang=Python"), "{text}");
        let block = &result["structuredContent"]["block_lines"];
        assert!(
            block
                .as_array()
                .unwrap()
                .iter()
                .all(|l| { !(l["content"].as_str() == Some("") && l["n"].as_u64().unwrap() > 4) }),
            "{block}"
        );
    }

    #[test]
    fn patch_surfaces_apply_time_warnings() {
        // A boundary-echo payload is *repaired* during apply — the repair
        // changes file content, so its warning must reach the MCP client
        // (previously it went only to the server's stderr and was lost).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();
        let file = path.to_str().unwrap();
        let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
        let anchor = format!("2:{}", read["lines"][1]["hash"].as_str().unwrap());
        // Payload restates the surviving lines above/below the range —
        // repair_boundary_echo drops both echoes.
        let patch = format!("SWAP {anchor}:\n+aaa\n+NEW\n+ccc");
        let result = call_tool(
            "patch",
            &json!({"file": file, "patch": patch, "json": true}),
        )
        .unwrap();
        assert!(result.get("isError").is_none(), "{result}");
        let payload: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        let warnings = payload["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains("Auto-repaired")),
            "{payload}"
        );
        assert_eq!(payload["changed"][0]["content"], json!("NEW"), "{payload}");
        // The repair actually happened on disk.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaa\nNEW\nccc\n");
    }

    #[test]
    fn patch_dry_run_is_bounded_and_json_has_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let content: String = (0..300).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &content).unwrap();
        let file = path.to_str().unwrap();
        let read: Value = serde_json::from_str(&handle_read(file, true).text).unwrap();
        let anchor = format!("150:{}", read["lines"][149]["hash"].as_str().unwrap());
        let patch = format!("SWAP {anchor}:\n+replaced line");

        // Compact dry-run: diff rows only — must not contain untouched lines.
        let preview = call_tool(
            "patch",
            &json!({"file": file, "patch": patch, "dry_run": true}),
        )
        .unwrap();
        let text = preview["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("+replaced line"), "{text}");
        assert!(
            !text.contains("line 0\n"),
            "dry-run dumped the file: {text}"
        );
        assert!(
            text.len() < 500,
            "dry-run output too large: {} bytes",
            text.len()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);

        // JSON dry-run: warnings array is always present.
        let preview = call_tool(
            "patch",
            &json!({"file": file, "patch": patch, "dry_run": true, "json": true}),
        )
        .unwrap();
        let payload: Value =
            serde_json::from_str(preview["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["dry_run"], json!(true), "{payload}");
        assert_eq!(payload["edits_applied"], json!(1), "{payload}");
        assert!(payload["warnings"].is_array(), "{payload}");
        assert!(payload["diff"].is_array(), "{payload}");
    }

    #[test]
    fn patch_json_updated_anchors_use_read_line_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let file = path.to_str().unwrap();
        let result = call_tool(
            "patch",
            &json!({
                "file": file,
                "patch": "INS.TAIL:\n+three",
                "return_updated_anchors": true,
                "json": true,
            }),
        )
        .unwrap();
        assert!(result.get("isError").is_none(), "{result}");
        let payload: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        let anchors = payload["updated_anchors"].as_array().unwrap();
        assert_eq!(anchors.len(), 3, "{payload}");
        // Same {n, hash, content} shape as `read --json` — not {line, text}.
        assert_eq!(anchors[0]["n"], json!(1), "{anchors:?}");
        assert_eq!(anchors[0]["content"], json!("one"), "{anchors:?}");
        assert!(anchors[0]["hash"].is_string(), "{anchors:?}");
        assert!(anchors[0].get("line").is_none(), "{anchors:?}");
        assert!(anchors[0].get("text").is_none(), "{anchors:?}");
    }

    #[test]
    fn patch_rejects_file_ops_instead_of_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "content\n").unwrap();
        let file = path.to_str().unwrap();
        // `REM`/`MV` file ops were parsed then silently discarded — the
        // client saw OK while nothing happened.
        for patch in ["REM", "MV other.txt"] {
            let result = call_tool("patch", &json!({"file": file, "patch": patch})).unwrap();
            assert_eq!(result["isError"], json!(true), "{patch}: {result}");
            assert_eq!(
                result["structuredContent"]["kind"],
                json!("PATCH_REJECTED"),
                "{patch}: {result}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "content\n");
        }
    }

    // ---- remove_file/rename_file CLI guard parity (bd-mcp-serve-parity) --

    #[test]
    fn remove_file_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("adir");
        std::fs::create_dir(&target).unwrap();
        let out = handle_remove_file(target.to_str().unwrap(), true);
        assert!(out.text.starts_with("Error:"), "{}", out.text);
        assert!(target.is_dir(), "directory must survive");
    }

    #[test]
    fn rename_file_applies_cli_guards() {
        let dir = tempfile::tempdir().unwrap();
        // Directory source: fs::rename would move the whole tree on Unix.
        let src_dir = dir.path().join("srcdir");
        std::fs::create_dir(&src_dir).unwrap();
        let out = handle_rename_file(
            src_dir.to_str().unwrap(),
            dir.path().join("dst").to_str().unwrap(),
            true,
        );
        assert!(out.text.starts_with("Error:"), "{}", out.text);
        assert!(src_dir.is_dir());

        // Existing destination: refused (no --force on the MCP tool).
        let src = dir.path().join("a.txt");
        let dst = dir.path().join("b.txt");
        std::fs::write(&src, "a").unwrap();
        std::fs::write(&dst, "b").unwrap();
        let out = handle_rename_file(src.to_str().unwrap(), dst.to_str().unwrap(), true);
        assert!(out.text.starts_with("Error:"), "{}", out.text);
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "b");

        // src == dst alias: refused before anything destructive.
        let out = handle_rename_file(
            src.to_str().unwrap(),
            dir.path().join(".").join("a.txt").to_str().unwrap(),
            true,
        );
        assert!(out.text.starts_with("Error:"), "{}", out.text);
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "a");

        // Happy path still works, including parent creation.
        let dst2 = dir.path().join("sub").join("dir").join("c.txt");
        let out = handle_rename_file(src.to_str().unwrap(), dst2.to_str().unwrap(), false);
        assert!(out.text.starts_with("OK"), "{}", out.text);
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dst2).unwrap(), "a");
    }
}
