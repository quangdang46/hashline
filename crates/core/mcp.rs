use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli::{
    AnnotateCmd, Commands, DeleteCmd, DoctorCmd, EditCmd, ExplodeCmd, FindBlockCmd, FromDiffCmd,
    GrepCmd, ImplodeCmd, IndentCmd, IndexCmd, InsertCmd, McpCmd, MergePatchesCmd, MoveCmd,
    PatchCmd, ReadCmd, StatsCmd, SwapCmd, VerifyCmd, WatchCapabilitiesCmd, WatchCmd, WorkflowsCmd,
};
use crate::document::{Document, FileMeta, FileStats, read_file_meta};
use crate::error::LinehashError;
use crate::orchestration::{
    annotate_lines, command_name, doctor_payload, grep_lines, index_payload, read_payload,
    verify_report, watch_capabilities_payload,
};
use crate::risk::{assess_command, blocked_assessment};
use crate::run_command;

const SERVER_INSTRUCTIONS: &str = "\
linehash MCP server. Use hash-anchored file operations when exact text edits are unsafe.\n\
\n\
Preferred workflow:\n\
1. For large or noisy files, do not start with a full-file linehash_read. Use linehash_index, linehash_annotate, or linehash_grep first.\n\
2. Once you know the target, call linehash_read with anchor plus small context for file-local snippet inspection.\n\
3. Use linehash_find_block when one tight snippet is not enough structural context.\n\
4. Call linehash_verify before risky grouped edits or when anchors may be stale.\n\
5. Use linehash_edit, linehash_insert, linehash_delete, or linehash_patch for mutations once anchors are known.\n\
6. Call linehash_workflows when you want a repo-local skill pack instead of reconstructing a linehash workflow from scratch.\n\
7. Use linehash_watch_capabilities before assuming MCP supports a streaming watch loop.\n\
\n\
Treat stale anchors as safety signals. Re-read and retry with fresh anchors instead of guessing. Prefer mutation tools over repeated exploratory reads once you have the right anchors.";

#[derive(Default)]
struct SessionState {
    docs: HashMap<PathBuf, CacheEntry>,
}

struct CacheEntry {
    meta: FileMeta,
    doc: Document,
    stats: Option<FileStats>,
}

impl SessionState {
    fn get(&mut self, path: &Path) -> Result<&mut CacheEntry, JsonRpcError> {
        let meta = read_file_meta(path).map_err(command_error)?;
        let key = path.to_path_buf();
        let needs_refresh = self.docs.get(&key).is_none_or(|entry| entry.meta != meta);

        if needs_refresh {
            let doc = Document::load(path).map_err(command_error)?;
            self.docs.insert(
                key.clone(),
                CacheEntry {
                    meta,
                    doc,
                    stats: None,
                },
            );
        }

        self.docs
            .get_mut(&key)
            .ok_or_else(|| tool_error(-32603, "session cache lookup failed", None))
    }

    fn invalidate(&mut self, path: &Path) {
        self.docs.remove(path);
    }
}

impl CacheEntry {
    fn stats(&mut self) -> &FileStats {
        self.stats.get_or_insert_with(|| self.doc.compute_stats())
    }
}

pub fn run(_cmd: McpCmd) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut session = SessionState::default();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_error(
                    &mut stdout,
                    None,
                    -32700,
                    &format!("parse error: {error}"),
                    None,
                )?;
                continue;
            }
        };

        if request.id.is_none() {
            continue;
        }

        let response = handle_request(&request, &mut session);
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn handle_request(request: &JsonRpcRequest, session: &mut SessionState) -> JsonRpcResponse {
    match request.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "linehash",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": SERVER_INSTRUCTIONS,
            })),
            error: None,
        },
        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: Some(json!({ "tools": tool_definitions() })),
            error: None,
        },
        "tools/call" => handle_tool_call(request, session),
        "ping" => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: Some(json!({})),
            error: None,
        },
        other => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method not found: {other}"),
                data: None,
            }),
        },
    }
}

fn handle_tool_call(request: &JsonRpcRequest, session: &mut SessionState) -> JsonRpcResponse {
    let params: ToolCallParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("invalid tool call params: {error}"),
                    data: None,
                }),
            };
        }
    };

    match dispatch_tool(&params.name, &params.arguments, session) {
        Ok(payload) => match serde_json::to_string_pretty(&payload) {
            Ok(text) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.clone(),
                result: Some(json!({
                    "content": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ],
                    "structuredContent": payload,
                })),
                error: None,
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.clone(),
                result: None,
                error: Some(tool_error(
                    -32603,
                    &format!("failed to serialize tool payload: {error}"),
                    None,
                )),
            },
        },
        Err(error) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: None,
            error: Some(error),
        },
    }
}

fn dispatch_tool(
    tool: &str,
    arguments: &Value,
    session: &mut SessionState,
) -> Result<Value, JsonRpcError> {
    match tool {
        "linehash_read" => tool_read(arguments, session),
        "linehash_index" => tool_index(arguments, session),
        "linehash_edit" => {
            let mut cmd: EditCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Edit(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_insert" => {
            let mut cmd: InsertCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Insert(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_delete" => {
            let mut cmd: DeleteCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Delete(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_verify" => tool_verify(arguments, session),
        "linehash_grep" => tool_grep(arguments, session),
        "linehash_annotate" => tool_annotate(arguments, session),
        "linehash_patch" => {
            let mut cmd: PatchCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Patch(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_swap" => {
            let cmd: SwapCmd = parse_args(arguments)?;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Swap(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_move" => {
            let cmd: MoveCmd = parse_args(arguments)?;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Move(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_indent" => {
            let mut cmd: IndentCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Indent(cmd));
            if result.is_ok() {
                session.invalidate(&path);
            }
            result
        }
        "linehash_find_block" => {
            let mut cmd: FindBlockCmd = parse_args(arguments)?;
            cmd.json = true;
            invoke_command(Commands::FindBlock(cmd))
        }
        "linehash_workflows" => {
            let mut cmd: WorkflowsCmd = parse_args(arguments)?;
            cmd.json = true;
            invoke_command(Commands::Workflows(cmd))
        }
        "linehash_watch_capabilities" => {
            let cmd = WatchCapabilitiesCmd { json: true };
            invoke_command(Commands::WatchCapabilities(cmd))
        }
        "linehash_stats" => tool_stats(arguments, session),
        "linehash_doctor" => tool_doctor(arguments, session),
        "linehash_from_diff" => {
            let mut cmd: FromDiffCmd = parse_args(arguments)?;
            cmd.json = true;
            invoke_command(Commands::FromDiff(cmd))
        }
        "linehash_merge_patches" => {
            let mut cmd: MergePatchesCmd = parse_args(arguments)?;
            cmd.json = true;
            invoke_command(Commands::MergePatches(cmd))
        }
        "linehash_watch" => {
            let mut cmd: WatchCmd = parse_args(arguments)?;
            if cmd.continuous {
                return Err(tool_error(
                    -32602,
                    "continuous watch is not supported over MCP; omit `continuous` or set `once=true`",
                    Some(json!({ "capabilities": watch_capabilities_payload() })),
                ));
            }
            cmd.once = true;
            cmd.json = true;
            let path = cmd.file.clone();
            let result = invoke_command(Commands::Watch(cmd));
            session.invalidate(&path);
            result
        }
        "linehash_explode" => {
            let cmd: ExplodeCmd = parse_args(arguments)?;
            invoke_command(Commands::Explode(cmd))
        }
        "linehash_implode" => {
            let cmd: ImplodeCmd = parse_args(arguments)?;
            let out = cmd.out.clone();
            let result = invoke_command(Commands::Implode(cmd));
            if result.is_ok() {
                session.invalidate(&out);
            }
            result
        }
        _ => Err(tool_error(-32601, &format!("unknown tool: {tool}"), None)),
    }
}

fn tool_read(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: ReadCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let data = read_payload(&entry.doc, &cmd.anchor, cmd.context).map_err(command_error)?;
    Ok(success_payload(
        "read",
        0,
        serde_json::to_value(data).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize read payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn tool_index(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: IndexCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    Ok(success_payload(
        "index",
        0,
        serde_json::to_value(index_payload(&entry.doc)).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize index payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn tool_grep(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: GrepCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let data = grep_lines(&entry.doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)
        .map_err(command_error)?;

    Ok(success_payload(
        "grep",
        0,
        serde_json::to_value(data).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize grep payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn tool_annotate(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: AnnotateCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let report =
        annotate_lines(&entry.doc, &cmd.query, cmd.regex, cmd.expect_one).map_err(command_error)?;

    Ok(success_payload(
        "annotate",
        report.exit_code,
        serde_json::to_value(report.lines).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize annotate payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn tool_verify(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: VerifyCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let report = verify_report(&entry.doc, &cmd.anchors);

    Ok(success_payload(
        "verify",
        report.exit_code,
        serde_json::to_value(report.results).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize verify payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn tool_stats(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: StatsCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let stats = serde_json::to_value(entry.stats()).map_err(|error| {
        tool_error(-32603, &format!("failed to serialize stats: {error}"), None)
    })?;
    Ok(success_payload("stats", 0, stats, true))
}

fn tool_doctor(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: DoctorCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;
    let stats = entry.stats().clone();
    let payload = doctor_payload(&cmd.file, &stats);
    Ok(success_payload(
        "doctor",
        0,
        serde_json::to_value(payload).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize doctor payload: {error}"),
                None,
            )
        })?,
        true,
    ))
}

fn parse_args<T: DeserializeOwned>(arguments: &Value) -> Result<T, JsonRpcError> {
    serde_json::from_value(arguments.clone()).map_err(|error| {
        tool_error(
            -32602,
            &format!("invalid arguments: {error}"),
            Some(json!({ "arguments": arguments })),
        )
    })
}

fn invoke_command(command: Commands) -> Result<Value, JsonRpcError> {
    let risk = assess_command(&command);
    let command_name = command_name(&command);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit_code = run_command(command, &mut stdout, &mut stderr).map_err(command_error)?;
    let stdout_text = String::from_utf8(stdout)
        .map_err(|error| tool_error(-32603, &format!("stdout was not utf-8: {error}"), None))?;
    let stderr_text = String::from_utf8(stderr)
        .map_err(|error| tool_error(-32603, &format!("stderr was not utf-8: {error}"), None))?;

    let mut payload = json!({
        "command": command_name,
        "exit_code": exit_code,
        "stdout": stdout_text,
        "stderr": stderr_text,
    });

    if let Ok(data) = serde_json::from_str::<Value>(payload["stdout"].as_str().unwrap_or("")) {
        payload["data"] = data;
    }
    if let Some(risk) = risk {
        payload["risk"] = serde_json::to_value(risk).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize risk payload: {error}"),
                None,
            )
        })?;
    }

    Ok(payload)
}

fn success_payload(command: &str, exit_code: i32, data: Value, cache_used: bool) -> Value {
    json!({
        "command": command,
        "exit_code": exit_code,
        "stdout": "",
        "stderr": "",
        "data": data,
        "cache": { "used": cache_used },
    })
}

fn command_error(error: LinehashError) -> JsonRpcError {
    let mut data = serde_json::Map::new();
    if let Some(hint) = error.hint() {
        data.insert("hint".into(), json!(hint));
    }
    if let Some(command) = error.command() {
        data.insert("command".into(), json!(command));
    }
    if let Some(risk) = blocked_assessment(&error) {
        data.insert("risk".into(), json!(risk));
    }

    tool_error(
        -32001,
        &error.to_string(),
        (!data.is_empty()).then_some(Value::Object(data)),
    )
}

fn write_error(
    stdout: &mut impl Write,
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> io::Result<()> {
    let response = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_owned(),
            data,
        }),
    };
    serde_json::to_writer(&mut *stdout, &response)?;
    stdout.write_all(b"\n")
}

fn tool_error(code: i32, message: &str, data: Option<Value>) -> JsonRpcError {
    JsonRpcError {
        code,
        message: message.to_owned(),
        data,
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "linehash_read",
            "Read a file with current line:hash anchors. For large or noisy files, prefer linehash_index, linehash_grep, or linehash_annotate first; then pass anchor with a small context for snippet-only inspection instead of a full-file read.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Path to the file."),
                    "anchor": array_schema("Optional anchors to focus snippet output. Use this for local inspection on large or noisy files."),
                    "context": integer_schema("Context lines around anchors. Defaults to 5; keep this tight for large or noisy files.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "linehash_index",
            "List anchors for every line without content. Prefer this as the first inspection step for large or noisy files when a full read would be too verbose.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Path to the file.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "linehash_grep",
            "Search file content and return matching lines with anchors. Prefer this before linehash_read when you know a pattern and need to localize the target without dumping the whole file.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Path to the file."),
                    "pattern": string_schema("Literal or regex pattern."),
                    "invert": bool_schema("Invert the match."),
                    "case_insensitive": bool_schema("Case-insensitive search.")
                },
                "required": ["file", "pattern"]
            }),
        ),
        tool(
            "linehash_annotate",
            "Map text or regex matches back to current anchors. Prefer this before linehash_read when you know the target text and want a precise anchor for snippet inspection or mutation.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Path to the file."),
                    "query": string_schema("Text or regex query."),
                    "regex": bool_schema("Treat query as regex."),
                    "expect_one": bool_schema("Require exactly one match.")
                },
                "required": ["file", "query"]
            }),
        ),
        tool(
            "linehash_verify",
            "Verify that one or more anchors still resolve. Use this before grouped edits or when another agent may have changed the file.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Path to the file."),
                    "anchors": array_schema("Anchors to verify.")
                },
                "required": ["file", "anchors"]
            }),
        ),
        tool(
            "linehash_edit",
            "Replace a single line or range at the specified anchor. Use this once anchors are known instead of describing a diff or doing more exploratory reads.",
            mutation_schema("anchor"),
        ),
        tool(
            "linehash_insert",
            "Insert content before or after an anchor. Use this once the insertion anchor is known instead of planning the change in prose.",
            json!({
                "type": "object",
                "properties": mutation_properties("anchor", true),
                "required": ["file", "anchor", "content"]
            }),
        ),
        tool(
            "linehash_delete",
            "Delete a line or range at the specified anchor. Use this once the target anchor is known instead of leaving deletion as a suggested diff.",
            json!({
                "type": "object",
                "properties": base_mutation_properties().into_iter().chain([
                    ("anchor".to_string(), string_schema("Anchor or range to delete."))
                ]).collect::<serde_json::Map<String, Value>>(),
                "required": ["file", "anchor"]
            }),
        ),
        tool(
            "linehash_patch",
            "Apply a JSON patch transaction atomically. Prefer this when several related mutations should happen together after you have collected anchors.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "patch": string_schema("Path to JSON patch file or '-' for stdin."),
                    "dry_run": bool_schema("Preview without writing."),
                    "receipt": bool_schema("Emit receipt JSON."),
                    "audit_log": string_schema("Optional audit log path."),
                    "expect_mtime": integer_schema("Optional expected mtime seconds."),
                    "expect_inode": integer_schema("Optional expected inode.")
                },
                "required": ["file", "patch"]
            }),
        ),
        tool(
            "linehash_swap",
            "Swap the positions of two anchored lines.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "anchor_a": string_schema("First anchor."),
                    "anchor_b": string_schema("Second anchor."),
                    "dry_run": bool_schema("Preview without writing."),
                    "receipt": bool_schema("Emit receipt JSON."),
                    "audit_log": string_schema("Optional audit log path."),
                    "expect_mtime": integer_schema("Optional expected mtime seconds."),
                    "expect_inode": integer_schema("Optional expected inode.")
                },
                "required": ["file", "anchor_a", "anchor_b"]
            }),
        ),
        tool(
            "linehash_move",
            "Move one anchored line before or after another anchor.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "anchor": string_schema("Source anchor."),
                    "direction": {
                        "type": "string",
                        "enum": ["before", "after"],
                        "description": "Placement relative to the target anchor."
                    },
                    "target": string_schema("Target anchor."),
                    "dry_run": bool_schema("Preview without writing."),
                    "receipt": bool_schema("Emit receipt JSON."),
                    "audit_log": string_schema("Optional audit log path."),
                    "expect_mtime": integer_schema("Optional expected mtime seconds."),
                    "expect_inode": integer_schema("Optional expected inode.")
                },
                "required": ["file", "anchor", "direction", "target"]
            }),
        ),
        tool(
            "linehash_indent",
            "Adjust indentation for a resolved range.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "range": string_schema("Qualified anchor range."),
                    "amount": string_schema("Indent delta like '+4' or '-2'."),
                    "dry_run": bool_schema("Preview without writing."),
                    "receipt": bool_schema("Emit receipt JSON."),
                    "audit_log": string_schema("Optional audit log path."),
                    "expect_mtime": integer_schema("Optional expected mtime seconds."),
                    "expect_inode": integer_schema("Optional expected inode.")
                },
                "required": ["file", "range", "amount"]
            }),
        ),
        tool(
            "linehash_workflows",
            "List repo-local markdown skill packs from `.linehash/skills` so agents can follow a bounded read/search/edit workflow instead of improvising command sequences.",
            json!({
                "type": "object",
                "properties": {
                    "root": string_schema("Workspace root that contains `.linehash/skills`. Defaults to the MCP server current working directory.")
                }
            }),
        ),
        tool(
            "linehash_watch_capabilities",
            "Explain the current watch capability split: the CLI supports continuous watch, while MCP supports only single-event watch calls today.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool(
            "linehash_find_block",
            "Find a likely structural block around an anchor.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "anchor": string_schema("Anchor inside the target block.")
                },
                "required": ["file", "anchor"]
            }),
        ),
        tool(
            "linehash_stats",
            "Compute collision and workflow guidance for a file.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "linehash_doctor",
            "Recommend the safest linehash workflow for a file.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "linehash_from_diff",
            "Convert a unified diff into anchor-aware operations.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "diff": string_schema("Path to unified diff file.")
                },
                "required": ["file", "diff"]
            }),
        ),
        tool(
            "linehash_merge_patches",
            "Merge two patch files against the same base file.",
            json!({
                "type": "object",
                "properties": {
                    "patch_a": string_schema("First patch file."),
                    "patch_b": string_schema("Second patch file."),
                    "base": string_schema("Base file path.")
                },
                "required": ["patch_a", "patch_b", "base"]
            }),
        ),
        tool(
            "linehash_watch",
            "Watch once for the next hash diff event on a file. Continuous mode is intentionally disabled over MCP.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path."),
                    "once": bool_schema("Ignored; MCP always performs a single event wait."),
                    "continuous": bool_schema("Must be false or omitted.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "linehash_explode",
            "Explode a file into per-line text files plus metadata.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Source file."),
                    "out": string_schema("Output directory."),
                    "force": bool_schema("Overwrite output directory if it exists.")
                },
                "required": ["file", "out"]
            }),
        ),
        tool(
            "linehash_implode",
            "Reassemble an exploded directory back into a file.",
            json!({
                "type": "object",
                "properties": {
                    "dir": string_schema("Exploded directory."),
                    "out": string_schema("Output file."),
                    "dry_run": bool_schema("Preview without writing.")
                },
                "required": ["dir", "out"]
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn string_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description,
    })
}

fn integer_schema(description: &str) -> Value {
    json!({
        "type": "integer",
        "description": description,
    })
}

fn bool_schema(description: &str) -> Value {
    json!({
        "type": "boolean",
        "description": description,
    })
}

fn array_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": {
            "type": "string"
        }
    })
}

fn base_mutation_properties() -> serde_json::Map<String, Value> {
    [
        ("file".to_string(), string_schema("Target file path.")),
        (
            "dry_run".to_string(),
            bool_schema("Preview without writing."),
        ),
        ("receipt".to_string(), bool_schema("Emit receipt JSON.")),
        (
            "audit_log".to_string(),
            string_schema("Optional audit log path."),
        ),
        (
            "expect_mtime".to_string(),
            integer_schema("Optional expected mtime seconds."),
        ),
        (
            "expect_inode".to_string(),
            integer_schema("Optional expected inode."),
        ),
    ]
    .into_iter()
    .collect()
}

fn mutation_properties(anchor_key: &str, include_before: bool) -> serde_json::Map<String, Value> {
    let mut properties = base_mutation_properties();
    properties.insert(
        anchor_key.to_string(),
        string_schema("Anchor or range identifying the target."),
    );
    properties.insert(
        "content".to_string(),
        string_schema("Replacement or inserted line content."),
    );
    if include_before {
        properties.insert(
            "before".to_string(),
            bool_schema("Insert before the anchor instead of after."),
        );
    }
    properties
}

fn mutation_schema(anchor_key: &str) -> Value {
    json!({
        "type": "object",
        "properties": mutation_properties(anchor_key, false),
        "required": ["file", anchor_key, "content"]
    })
}

#[cfg(test)]
mod tests {
    use super::{SERVER_INSTRUCTIONS, SessionState, dispatch_tool, tool_definitions};
    use anyhow::{Result, anyhow};
    use serde_json::Value;
    use serde_json::json;
    use std::fmt::Debug;
    use std::path::Path;
    use tempfile::TempDir;

    fn must<T, E: Debug>(result: std::result::Result<T, E>) -> Result<T> {
        result.map_err(|error| anyhow!("{error:?}"))
    }

    fn must_err<T, E: Debug>(result: std::result::Result<T, E>) -> Result<E> {
        match result {
            Ok(_) => Err(anyhow!("expected error")),
            Err(error) => Ok(error),
        }
    }

    fn json_u64(value: &Value) -> Result<u64> {
        match value.as_u64() {
            Some(number) => Ok(number),
            None => Err(anyhow!("expected JSON number, got {value}")),
        }
    }

    fn json_str<'a>(value: &'a Value) -> Result<&'a str> {
        match value.as_str() {
            Some(text) => Ok(text),
            None => Err(anyhow!("expected JSON string, got {value}")),
        }
    }

    fn json_array(value: &Value) -> Result<&[Value]> {
        match value.as_array() {
            Some(lines) => Ok(lines),
            None => Err(anyhow!("expected JSON array, got {value}")),
        }
    }

    fn line_anchor(read: &Value, index: usize) -> Result<String> {
        Ok(format!(
            "{}:{}",
            json_u64(&read["data"]["lines"][index]["n"])?,
            json_str(&read["data"]["lines"][index]["hash"])?
        ))
    }

    fn write_text(path: &Path, content: &str) -> Result<()> {
        std::fs::write(path, content).map_err(|error| anyhow!("{error}"))
    }

    fn read_text(path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|error| anyhow!("{error}"))
    }

    #[test]
    fn read_tool_returns_structured_json() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\n")?;

        let result = dispatch_tool(
            "linehash_read",
            &json!({
                "file": path,
            }),
            &mut SessionState::default(),
        );
        let result = must(result)?;

        assert_eq!(result["command"], "read");
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["data"]["lines"][0]["content"], "alpha");
        Ok(())
    }

    #[test]
    fn read_tool_honors_anchor_and_context_for_snippets() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\ndelta\nepsilon\n")?;
        let mut session = SessionState::default();

        let read = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let anchor = line_anchor(&read, 2)?;

        let snippet = dispatch_tool(
            "linehash_read",
            &json!({
                "file": path,
                "anchor": [anchor],
                "context": 1,
            }),
            &mut session,
        );
        let snippet = must(snippet)?;

        let lines = json_array(&snippet["data"]["lines"])?;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["content"], "beta");
        assert_eq!(lines[1]["content"], "gamma");
        assert_eq!(lines[2]["content"], "delta");
        Ok(())
    }

    #[test]
    fn tool_definitions_push_models_toward_targeted_mutations() -> Result<()> {
        let tools = tool_definitions();
        let read = tools
            .iter()
            .find(|tool| tool["name"] == "linehash_read")
            .ok_or_else(|| anyhow!("missing linehash_read tool"))?;
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "linehash_edit")
            .ok_or_else(|| anyhow!("missing linehash_edit tool"))?;
        let delete = tools
            .iter()
            .find(|tool| tool["name"] == "linehash_delete")
            .ok_or_else(|| anyhow!("missing linehash_delete tool"))?;

        assert!(
            read["description"]
                .as_str()
                .map_or(false, |text| text.contains("prefer linehash_index"))
        );
        assert!(
            edit["description"]
                .as_str()
                .map_or(false, |text| text.contains("once anchors are known"))
        );
        assert!(delete["description"].as_str().map_or(false, |text| {
            text.contains("instead of leaving deletion as a suggested diff")
        }));
        assert!(SERVER_INSTRUCTIONS.contains("do not start with a full-file linehash_read"));
        assert!(
            SERVER_INSTRUCTIONS
                .contains("Use linehash_edit, linehash_insert, linehash_delete, or linehash_patch")
        );
        Ok(())
    }

    #[test]
    fn watch_tool_rejects_continuous_mode() -> Result<()> {
        let error = dispatch_tool(
            "linehash_watch",
            &json!({
                "file": "demo.txt",
                "continuous": true,
            }),
            &mut SessionState::default(),
        );
        let error = must_err(error)?;

        assert_eq!(error.code, -32602);
        assert!(error.message.contains("continuous watch"));
        assert_eq!(
            error
                .data
                .as_ref()
                .map(|data| &data["capabilities"]["mcp_streaming_supported"]),
            Some(&json!(false))
        );
        Ok(())
    }

    #[test]
    fn read_tool_cache_refreshes_after_file_change() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n")?;
        let mut session = SessionState::default();

        let first = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        assert_eq!(first["data"]["lines"][0]["content"], "alpha");

        write_text(&path, "beta\n")?;

        let second = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        assert_eq!(second["data"]["lines"][0]["content"], "beta");
        Ok(())
    }

    #[test]
    fn edit_tool_accepts_multiline_content_for_range_anchor() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\ndelta\n")?;
        let mut session = SessionState::default();

        let read = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;

        let result = dispatch_tool(
            "linehash_edit",
            &json!({
                "file": path,
                "anchor": format!("{start}..{end}"),
                "content": "left\nmiddle\nright",
            }),
            &mut session,
        );
        let result = must(result)?;

        assert_eq!(result["exit_code"], 0);
        assert_eq!(read_text(&path)?, "alpha\nleft\nmiddle\nright\ndelta\n");
        Ok(())
    }

    #[test]
    fn delete_tool_accepts_range_anchor() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\ndelta\n")?;
        let mut session = SessionState::default();

        let read = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;

        let result = dispatch_tool(
            "linehash_delete",
            &json!({
                "file": path,
                "anchor": format!("{start}..{end}"),
            }),
            &mut session,
        );
        let result = must(result)?;

        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["risk"]["level"], "high");
        assert!(
            result["risk"]["summary"]
                .as_str()
                .map_or(false, |text| text.contains("permanently"))
        );
        assert_eq!(read_text(&path)?, "alpha\ndelta\n");
        Ok(())
    }

    #[test]
    fn edit_tool_reports_range_hint_for_dash_separated_qualified_range() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\ndelta\n")?;
        let mut session = SessionState::default();

        let read = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;
        let dashed_range = format!("{start}-{end}");

        let error = must_err(dispatch_tool(
            "linehash_edit",
            &json!({
                "file": path,
                "anchor": dashed_range,
                "content": "left\nmiddle\nright",
            }),
            &mut session,
        ))?;

        assert_eq!(error.code, -32001);
        assert!(error.message.contains("invalid range anchor"));
        assert_eq!(
            error.data.map(|data| data["hint"].clone()),
            Some(json!("use a range like '2:f1..4:9c'"))
        );
        Ok(())
    }

    #[test]
    fn stale_anchor_error_includes_blocked_risk_context() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\n")?;
        let mut session = SessionState::default();

        let read = must(dispatch_tool(
            "linehash_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let anchor = line_anchor(&read, 1)?;
        write_text(&path, "alpha\ngamma\nbeta\n")?;

        let error = must_err(dispatch_tool(
            "linehash_edit",
            &json!({
                "file": path,
                "anchor": anchor,
                "content": "BETA",
            }),
            &mut session,
        ))?;

        assert_eq!(error.code, -32001);
        assert_eq!(
            error
                .data
                .as_ref()
                .map(|data| data["risk"]["level"].clone()),
            Some(json!("blocked"))
        );
        assert!(
            error
                .data
                .as_ref()
                .and_then(|data| data["risk"]["summary"].as_str())
                .is_some_and(|text| text.contains("blocked"))
        );
        Ok(())
    }

    #[test]
    fn workflows_tool_returns_repo_skill_pack_catalog() -> Result<()> {
        let dir = must(TempDir::new())?;
        let skills_dir = dir.path().join(".linehash/skills/anchored-read");
        std::fs::create_dir_all(&skills_dir).map_err(|error| anyhow!("{error}"))?;
        std::fs::write(
            skills_dir.join("SKILL.md"),
            concat!(
                "---\n",
                "title = \"Anchored read\"\n",
                "description = \"Inspect before mutating\"\n",
                "surfaces = [\"local\", \"mcp\"]\n",
                "allowed_cli_commands = [\"linehash index\", \"linehash read\"]\n",
                "allowed_mcp_tools = [\"linehash_index\", \"linehash_read\"]\n",
                "---\n",
                "Use index first, then a focused read.\n",
            ),
        )
        .map_err(|error| anyhow!("{error}"))?;

        let result = must(dispatch_tool(
            "linehash_workflows",
            &json!({ "root": dir.path() }),
            &mut SessionState::default(),
        ))?;

        assert_eq!(result["command"], "workflows");
        assert_eq!(result["data"]["packs"][0]["name"], "anchored-read");
        assert_eq!(
            result["data"]["packs"][0]["allowed_mcp_tools"][0],
            "linehash_index"
        );
        Ok(())
    }

    #[test]
    fn watch_capabilities_tool_reports_mcp_limitations() -> Result<()> {
        let result = must(dispatch_tool(
            "linehash_watch_capabilities",
            &json!({}),
            &mut SessionState::default(),
        ))?;

        assert_eq!(result["command"], "watch-capabilities");
        assert_eq!(result["data"]["cli_continuous_supported"], true);
        assert_eq!(result["data"]["mcp_streaming_supported"], false);
        Ok(())
    }
}
