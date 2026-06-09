use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli::{
    AnnotateCmd, Commands, DeleteCmd, DoctorCmd, EditCmd, GrepCmd, IndentCmd, IndexCmd, InsertCmd,
    McpCmd, MoveCmd, PatchCmd, ReadCmd, StatsCmd, SwapCmd, VerifyCmd,
};
use crate::document::Document;
use crate::error::HashlineError;
use crate::orchestration::run_command;
use crate::orchestration::{
    command_name, doctor_payload, index_payload, read_payload, verify_report,
};
use crate::session_cache::{CacheStats, SessionCache};
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, EventKind, PollWatcher, RecursiveMode, Watcher};

use crate::hash::full_hash_bytes;
use crate::risk::{assess_command, blocked_assessment};

const SERVER_INSTRUCTIONS: &str = "\
hashline MCP server. Use hash-anchored file operations when exact text edits are unsafe.\n\
\n\
Preferred workflow:\n\
1. For large or noisy files, do not start with a full-file hashline_read. Use hashline_index, hashline_annotate, or hashline_grep first.\n\
2. Once you know the target, call hashline_read with anchor plus small context for file-local snippet inspection.\n\
3. Use hashline_find_block when one tight snippet is not enough structural context.\n\
4. Call hashline_verify before risky grouped edits or when anchors may be stale.\n\
5. Use hashline_edit, hashline_insert, hashline_delete, or hashline_patch for mutations once anchors are known.\n\
6. Use hashline_watch_capabilities before assuming MCP supports a streaming watch loop.\n\
\n\
Treat stale anchors as safety signals. Re-read and retry with fresh anchors instead of guessing. Prefer mutation tools over repeated exploratory reads once you have the right anchors.";

/// Create a fresh session for a daemon connection. Each connection gets
/// its own SessionCache so concurrent requests don't share mutable state.
pub fn new_session() -> SessionCache {
    SessionCache::new(128)
}

pub fn run(cmd: McpCmd) -> io::Result<()> {
    if cmd.proxy_to_daemon {
        return run_proxy();
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut session = SessionCache::new(128);

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

/// Proxy MCP stdin/stdout to a daemon via Unix socket.
/// Forwards each JSON-RPC line to the daemon and writes the response back.
fn run_proxy() -> io::Result<()> {
    let socket_path = crate::commands::serve::default_daemon_socket();
    let stream = std::os::unix::net::UnixStream::connect(&socket_path).map_err(|e| {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("cannot connect to daemon at {}: {e}", socket_path.display()),
        )
    })?;

    let read_stream = stream
        .try_clone()
        .map_err(|e| io::Error::other(format!("failed to clone socket: {e}")))?;
    let mut reader = io::BufReader::new(read_stream);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        // Forward to daemon
        (&stream).write_all(line.as_bytes())?;
        (&stream).write_all(b"\n")?;
        (&stream).flush()?;

        // Read response
        let mut response = String::new();
        reader.read_line(&mut response)?;

        // Write to stdout
        stdout.write_all(response.as_bytes())?;
        stdout.flush()?;
    }

    Ok(())
}

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    #[serde(rename = "jsonrpc")]
    pub _jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

pub(crate) fn handle_request(
    request: &JsonRpcRequest,
    session: &mut SessionCache,
) -> JsonRpcResponse {
    match request.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0",
            id: request.id.clone(),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "hashline",
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

pub(crate) fn handle_tool_call(
    request: &JsonRpcRequest,
    session: &mut SessionCache,
) -> JsonRpcResponse {
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
        // Compact JSON: tools' text content is consumed by the LLM, not humans.
        // Compact format saves ~20-30% tokens vs pretty (no indentation/newlines).
        // The `structuredContent` field still provides the parsed object form.
        Ok(payload) => match serde_json::to_string(&payload) {
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

pub fn dispatch_tool(
    tool: &str,
    arguments: &Value,
    session: &mut SessionCache,
) -> Result<Value, JsonRpcError> {
    match tool {
        "hashline_read" => tool_read(arguments, session),
        "hashline_index" => tool_index(arguments, session),
        "hashline_edit" => {
            let mut cmd: EditCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Edit(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_insert" => {
            let mut cmd: InsertCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Insert(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_delete" => {
            let mut cmd: DeleteCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Delete(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_verify" => tool_verify(arguments, session),
        "hashline_patch" => {
            let mut cmd: PatchCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Patch(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_swap" => {
            let cmd: SwapCmd = parse_args(arguments)?;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Swap(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_move" => {
            let cmd: MoveCmd = parse_args(arguments)?;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Move(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_indent" => {
            let mut cmd: IndentCmd = parse_args(arguments)?;
            cmd.json = true;
            let path = cmd.file.clone();
            let (payload, modified_doc) = invoke_command(Commands::Indent(cmd))?;
            if let Some(doc) = modified_doc {
                session.after_mutation(&path, doc);
            } else {
                session.invalidate(&path);
            }
            Ok(payload)
        }
        "hashline_stats" => tool_stats(arguments, session),
        "hashline_doctor" => tool_doctor(arguments, session),
        "hashline_grep" => tool_grep(arguments, session),
        "hashline_annotate" => tool_annotate(arguments, session),
        "hashline_explode" => tool_explode(arguments),
        "hashline_implode" => tool_implode(arguments),
        "hashline_watch_capabilities" => tool_watch_capabilities(),
        "hashline_watch" => tool_watch(arguments),
        "hashline_map" => tool_map(arguments),
        "hashline_symbol" => tool_symbol(arguments),
        "hashline_callees" => tool_callees(arguments),
        "hashline_from_diff" => tool_from_diff(arguments, session),
        "hashline_merge_patches" => tool_merge_patches(arguments, session),
        _ => Err(tool_error(-32601, &format!("unknown tool: {tool}"), None)),
    }
}

fn tool_read(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: ReadCmd = parse_args(arguments)?;
    session.set_no_cache(cmd.no_cache);
    let entry = session.get_or_load(&cmd.file).map_err(command_error)?;
    let data = read_payload(entry.doc(), &cmd.anchor, cmd.context).map_err(command_error)?;
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
        session.stats(),
    ))
}

fn tool_index(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: IndexCmd = parse_args(arguments)?;
    session.set_no_cache(cmd.no_cache);
    let entry = session.get_or_load(&cmd.file).map_err(command_error)?;
    Ok(success_payload(
        "index",
        0,
        serde_json::to_value(index_payload(entry.doc())).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize index payload: {error}"),
                None,
            )
        })?,
        session.stats(),
    ))
}

fn tool_verify(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: VerifyCmd = parse_args(arguments)?;
    session.set_no_cache(cmd.no_cache);
    let entry = session.get_or_load(&cmd.file).map_err(command_error)?;
    let report = verify_report(entry.doc(), &cmd.anchors);

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
        session.stats(),
    ))
}

fn tool_stats(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: StatsCmd = parse_args(arguments)?;
    session.set_no_cache(cmd.no_cache);
    let entry = session.get_or_load(&cmd.file).map_err(command_error)?;
    let stats = serde_json::to_value(entry.stats()).map_err(|error| {
        tool_error(-32603, &format!("failed to serialize stats: {error}"), None)
    })?;
    Ok(success_payload("stats", 0, stats, session.stats()))
}

fn tool_doctor(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: DoctorCmd = parse_args(arguments)?;
    session.set_no_cache(cmd.no_cache);
    let entry = session.get_or_load(&cmd.file).map_err(command_error)?;
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
        session.stats(),
    ))
}

fn tool_grep(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: GrepCmd = parse_args(arguments)?;
    // Use the session cache to validate the file exists and validate its hash,
    // but we need to actually search the file content. Load a SearchDocument
    // distinct from the cached Document since SearchDocument has the line
    // offsets we need for iteration.
    session.get_or_load(&cmd.file).map_err(command_error)?;

    let search_doc = crate::document::SearchDocument::load(&cmd.file).map_err(command_error)?;

    let regex = regex::RegexBuilder::new(&cmd.pattern)
        .case_insensitive(cmd.case_insensitive)
        .build()
        .map_err(|e| {
            tool_error(
                -32602,
                &format!("invalid pattern '{}': {}", cmd.pattern, e),
                None,
            )
        })?;

    let mut results = Vec::new();

    for (line_idx, &start) in search_doc.line_offsets.iter().enumerate() {
        let end = if line_idx + 1 < search_doc.line_offsets.len() {
            search_doc.line_offsets[line_idx + 1]
        } else {
            search_doc.content.len()
        };
        let line_end = if search_doc.trailing_newline
            && end > start
            && search_doc.content.as_bytes()[end.saturating_sub(1)] == b'\n'
        {
            end - 1
        } else {
            end.min(search_doc.content.len())
        };
        let line_content = search_doc.content[start..line_end]
            .strip_suffix('\r')
            .unwrap_or(&search_doc.content[start..line_end]);

        let is_match = regex.is_match(line_content);
        let include = if cmd.invert { !is_match } else { is_match };

        if include {
            let fh = crate::hash::full_hash(line_content);
            let sh = crate::hash::short_from_full(fh);
            results.push(crate::document::LineView {
                n: line_idx + 1,
                hash: crate::hash::format_short_hash(sh),
                content: line_content.to_string(),
            });
        }
    }

    Ok(success_payload(
        "grep",
        0,
        serde_json::to_value(results).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize grep results: {error}"),
                None,
            )
        })?,
        session.stats(),
    ))
}

fn tool_annotate(arguments: &Value, session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let cmd: AnnotateCmd = parse_args(arguments)?;
    session.get_or_load(&cmd.file).map_err(command_error)?;

    let search_doc = crate::document::SearchDocument::load(&cmd.file).map_err(command_error)?;

    let results = if cmd.regex {
        let re = regex::RegexBuilder::new(&cmd.query)
            .case_insensitive(false)
            .build()
            .map_err(|e| {
                tool_error(
                    -32602,
                    &format!("invalid query '{}': {}", cmd.query, e),
                    None,
                )
            })?;

        let mut results = Vec::new();
        for (line_idx, &start) in search_doc.line_offsets.iter().enumerate() {
            let end = if line_idx + 1 < search_doc.line_offsets.len() {
                search_doc.line_offsets[line_idx + 1]
            } else {
                search_doc.content.len()
            };
            let line_end = if search_doc.trailing_newline
                && end > start
                && search_doc.content.as_bytes()[end.saturating_sub(1)] == b'\n'
            {
                end - 1
            } else {
                end.min(search_doc.content.len())
            };
            let line_content = search_doc.content[start..line_end]
                .strip_suffix('\r')
                .unwrap_or(&search_doc.content[start..line_end]);

            if re.is_match(line_content) {
                let fh = crate::hash::full_hash(line_content);
                let sh = crate::hash::short_from_full(fh);
                results.push(crate::document::LineView {
                    n: line_idx + 1,
                    hash: crate::hash::format_short_hash(sh),
                    content: line_content.to_string(),
                });
            }
        }
        results
    } else {
        let mut results = Vec::new();
        search_doc.grep_for_each(&cmd.query, false, |line_idx, content, short_hash| {
            results.push(crate::document::LineView {
                n: line_idx + 1,
                hash: crate::hash::format_short_hash(short_hash),
                content: content.to_string(),
            });
        });
        results
    };

    if cmd.expect_one && results.len() != 1 {
        let msg = if results.is_empty() {
            format!(
                "expected exactly 1 match for query '{}', but found 0",
                cmd.query
            )
        } else {
            format!(
                "expected exactly 1 match for query '{}', but found {}",
                cmd.query,
                results.len()
            )
        };
        return Err(tool_error(-32001, &msg, None));
    }

    Ok(success_payload(
        "annotate",
        0,
        serde_json::to_value(results).map_err(|error| {
            tool_error(
                -32603,
                &format!("failed to serialize annotate results: {error}"),
                None,
            )
        })?,
        session.stats(),
    ))
}

fn tool_explode(arguments: &Value) -> Result<Value, JsonRpcError> {
    let file: String = parse_arg(arguments, "file")?;
    let out: String = parse_arg(arguments, "out")?;
    let force: bool = arguments
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let out_path = Path::new(&out);
    if out_path.exists() {
        if !force {
            return Err(command_error(HashlineError::ExplodeTargetExists {
                path: out,
            }));
        }
        std::fs::remove_dir_all(out_path).map_err(|e| {
            tool_error(
                -32603,
                &format!("failed to remove existing output directory: {e}"),
                None,
            )
        })?;
    }

    let content =
        std::fs::read_to_string(&file).map_err(|e| command_error(HashlineError::Io(e)))?;

    // Detect newline style
    let (newline_style, lines_raw): (&str, Vec<&str>) = if content.contains("\r\n") {
        ("crlf", content.split("\r\n").collect())
    } else if content.contains('\r') {
        ("cr", content.split('\r').collect())
    } else {
        ("lf", content.split('\n').collect())
    };

    let trailing_newline = content.ends_with('\n');
    let line_count = if trailing_newline && !content.is_empty() {
        lines_raw.len().saturating_sub(1)
    } else {
        lines_raw.len()
    };

    // If empty file with trailing newline, lines_raw is [""] and line_count is 0
    let effective_lines: Vec<&str> = if line_count == 0 && content.is_empty() {
        vec![]
    } else if trailing_newline && !content.is_empty() {
        lines_raw[..lines_raw.len().saturating_sub(1)].to_vec()
    } else if trailing_newline && content == "\n" {
        // just a newline
        vec![""]
    } else {
        lines_raw[..line_count].to_vec()
    };

    std::fs::create_dir_all(out_path).map_err(|e| command_error(HashlineError::Io(e)))?;

    for (i, line) in effective_lines.iter().enumerate() {
        let line_file = out_path.join(format!("L{}", i + 1));
        std::fs::write(&line_file, line).map_err(|e| command_error(HashlineError::Io(e)))?;
    }

    // Compute content hash over the raw file bytes
    let content_hash = format!("{:08x}", full_hash_bytes(content.as_bytes()));

    let meta = json!({
        "original": file,
        "line_count": line_count,
        "newline_style": newline_style,
        "trailing_newline": trailing_newline,
        "content_hash": content_hash,
    });

    let meta_path = out_path.join(".meta.json");
    std::fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta)
            .map_err(|e| tool_error(-32603, &format!("failed to serialize meta: {e}"), None))?,
    )
    .map_err(|e| command_error(HashlineError::Io(e)))?;

    Ok(success_payload("explode", 0, meta, &CacheStats::default()))
}

fn tool_implode(arguments: &Value) -> Result<Value, JsonRpcError> {
    let dir: String = parse_arg(arguments, "dir")?;
    let out: String = parse_arg(arguments, "out")?;
    let dry_run: bool = arguments
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let dir_path = Path::new(&dir);

    // Read and validate .meta.json
    let meta_path = dir_path.join(".meta.json");
    let meta_content = std::fs::read_to_string(&meta_path)
        .map_err(|_| command_error(HashlineError::ImplodeMissingMeta { path: dir.clone() }))?;

    let meta: serde_json::Value = serde_json::from_str(&meta_content).map_err(|e| {
        command_error(HashlineError::ImplodeInvalidMeta {
            path: meta_path.to_string_lossy().to_string(),
            reason: format!("invalid JSON: {e}"),
        })
    })?;

    let line_count = meta
        .get("line_count")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            command_error(HashlineError::ImplodeInvalidMeta {
                path: meta_path.to_string_lossy().to_string(),
                reason: "missing or invalid 'line_count'".into(),
            })
        })? as usize;

    let newline_style = meta
        .get("newline_style")
        .and_then(|v| v.as_str())
        .unwrap_or("lf");

    let _trailing_newline = meta
        .get("trailing_newline")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Validate the directory: no unexpected files
    let mut entries: Vec<_> = std::fs::read_dir(dir_path)
        .map_err(|e| command_error(HashlineError::Io(e)))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".meta.json" {
            continue;
        }
        if name_str.starts_with('L') {
            // It's a line file — validate it's numeric after L
            let num_part = name_str.strip_prefix('L').unwrap_or(&name_str);
            if num_part.parse::<usize>().is_ok() {
                continue;
            }
        }
        return Err(command_error(HashlineError::ImplodeDirtyDirectory {
            path: dir.clone(),
            entry: name_str.to_string(),
        }));
    }

    // Reassemble lines
    let line_separator = match newline_style {
        "crlf" => "\r\n",
        "cr" => "\r",
        _ => "\n",
    };

    let mut reassembled = String::new();
    for i in 1..=line_count {
        let line_path = dir_path.join(format!("L{i}"));
        let line_content = std::fs::read_to_string(&line_path).map_err(|_| {
            command_error(HashlineError::ImplodeMissingLineFile {
                path: dir.clone(),
                line_no: i,
            })
        })?;
        if i > 1 {
            reassembled.push_str(line_separator);
        }
        reassembled.push_str(&line_content);
    }

    // Validate content hash if present
    let hash_mismatch = meta
        .get("content_hash")
        .and_then(|v| v.as_str())
        .map(|expected_hash| {
            let actual_hash = format!("{:08x}", full_hash_bytes(reassembled.as_bytes()));
            actual_hash != expected_hash
        })
        .unwrap_or(false);

    let result = json!({
        "line_count": line_count,
        "newline_style": newline_style,
        "hash_mismatch": hash_mismatch,
    });

    if dry_run {
        return Ok(success_payload(
            "implode",
            0,
            result,
            &CacheStats::default(),
        ));
    }

    // Fix trailing newline: the reassembled content does NOT include a trailing newline.
    // Add one if the original had it.
    if _trailing_newline {
        reassembled.push_str(line_separator);
    }

    std::fs::write(&out, &reassembled).map_err(|e| command_error(HashlineError::Io(e)))?;

    Ok(success_payload(
        "implode",
        0,
        result,
        &CacheStats::default(),
    ))
}

fn tool_watch_capabilities() -> Result<Value, JsonRpcError> {
    let text = "hashline_watch on the CLI supports continuous (streaming) watch. Over MCP, only single-event watch is supported: hashline_watch waits for the next modification event and returns immediately. For continuous watching, use the CLI command `hashline watch <file>`.";

    Ok(json!({
        "command": "watch_capabilities",
        "exit_code": 0,
        "stdout": "",
        "stderr": "",
        "data": null,
        "watch_capabilities": text,
        "cache": { "used": false },
    }))
}

fn tool_watch(arguments: &Value) -> Result<Value, JsonRpcError> {
    let file: String = parse_arg(arguments, "file")?;
    let continuous: bool = arguments
        .get("continuous")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if continuous {
        return Err(tool_error(
            -32602,
            "continuous mode is not supported over MCP. Set continuous to false or omit it.",
            None,
        ));
    }

    let file_path = Path::new(&file);

    // Verify file exists
    if !file_path.exists() {
        return Err(tool_error(
            -32001,
            &format!("file does not exist: {file}"),
            None,
        ));
    }

    let (tx, rx) = mpsc::channel();

    let mut watcher = PollWatcher::new(
        tx,
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )
    .map_err(|e| tool_error(-32603, &format!("failed to create PollWatcher: {e}"), None))?;

    watcher
        .watch(file_path, RecursiveMode::NonRecursive)
        .map_err(|e| tool_error(-32603, &format!("failed to watch file: {e}"), None))?;

    // Wait for one Modify event, timeout after 60 seconds
    let timeout = Duration::from_secs(60);
    let deadline = std::time::Instant::now() + timeout;

    let event_opt = loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .unwrap_or_default();

        if remaining.is_zero() {
            break None;
        }

        match rx.recv_timeout(remaining) {
            Ok(Ok(event)) => {
                if matches!(event.kind, EventKind::Modify(_)) {
                    break Some(event);
                }
                // Ignore non-modify events and continue waiting
            }
            Ok(Err(e)) => {
                return Err(tool_error(-32603, &format!("watch error: {e}"), None));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                break None;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break None;
            }
        }
    };

    // Drop watcher to stop watching
    drop(watcher);

    match event_opt {
        Some(event) => {
            let paths: Vec<String> = event
                .paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let kind = format!("{:?}", event.kind);
            Ok(json!({
                "command": "watch",
                "exit_code": 0,
                "stdout": "",
                "stderr": "",
                "data": {
                    "kind": kind,
                    "paths": paths,
                    "file": file,
                },
                "cache": { "used": false },
            }))
        }
        None => Ok(json!({
            "command": "watch",
            "exit_code": 0,
            "stdout": "",
            "stderr": "",
            "data": {
                "kind": null,
                "paths": [],
                "file": file,
                "message": "no change detected within 60 seconds",
            },
            "cache": { "used": false },
        })),
    }
}

fn tool_map(arguments: &Value) -> Result<Value, JsonRpcError> {
    let scope: String = arguments
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ".".to_string());
    let depth = arguments
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize);
    let budget = arguments
        .get("budget")
        .and_then(|v| v.as_u64())
        .map(|b| b as usize);
    let json_out = arguments
        .get("json")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let root = std::path::Path::new(&scope);
    if !root.is_dir() {
        return Err(tool_error(
            -32001,
            &format!("not a directory: {scope}"),
            None,
        ));
    }

    let root_canonical = root
        .canonicalize()
        .map_err(|e| tool_error(-32603, &format!("failed to canonicalize path: {e}"), None))?
        .to_string_lossy()
        .to_string();

    // First pass: collect all entries with their stats
    let mut dirs: Vec<(PathBuf, u32)> = Vec::new();
    let mut files: Vec<(PathBuf, u64)> = Vec::new();

    let walk = walkdir::WalkDir::new(root)
        .max_depth(depth.unwrap_or(usize::MAX))
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden files/dirs (starting with '.') unless it's root
            e.file_name()
                .to_str()
                .map(|s| s.starts_with('.') == (e.depth() == 0))
                .unwrap_or(true)
        });

    for entry in walk {
        let entry = entry.map_err(|e| tool_error(-32603, &format!("walk error: {e}"), None))?;
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path().to_path_buf();
        if entry.file_type().is_dir() {
            dirs.push((path, entry.depth() as u32));
        } else if entry.file_type().is_file() {
            // Estimate tokens: read file, chars / 4
            let tokens = estimate_file_tokens(&path);
            files.push((path, tokens));
        }
    }

    // Build tree: group files under their parent dirs
    // We'll compute total_tokens per directory recursively
    let mut dir_totals: HashMap<PathBuf, (u64, usize)> = HashMap::new(); // path -> (file_count, total_tokens)

    // Collect all parent dirs of files
    for (file_path, tokens) in &files {
        if let Some(parent) = file_path.parent() {
            let entry = dir_totals.entry(parent.to_path_buf()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += *tokens as usize;
        }
    }

    // Propagate child dir totals to parent dirs
    // Sort by depth descending so we process deepest first
    let mut all_dirs: Vec<(PathBuf, u32)> = dirs.clone();
    // Add parent dirs of files that aren't in dirs already
    for dir_path in dir_totals.keys() {
        if !all_dirs.iter().any(|(p, _)| p == dir_path) {
            let depth_val = dir_path
                .strip_prefix(root)
                .map(|p| p.components().count())
                .unwrap_or(0) as u32;
            all_dirs.push((dir_path.clone(), depth_val));
        }
    }
    // Also add root
    all_dirs.push((root.to_path_buf(), 0));

    all_dirs.sort_by_cached_key(|a| std::cmp::Reverse(a.1)); // deepest first

    for (dir_path, _) in &all_dirs {
        if *dir_path == root {
            continue;
        }
        let my_total = dir_totals.get(dir_path).copied().unwrap_or((0, 0));
        if let Some(parent) = dir_path.parent() {
            let entry = dir_totals.entry(parent.to_path_buf()).or_insert((0, 0));
            entry.0 += my_total.0;
            entry.1 += my_total.1;
        }
    }

    let total_files = dir_totals
        .get(root)
        .map(|(c, _)| *c)
        .unwrap_or(files.len() as u64);
    let total_tokens = dir_totals.get(root).map(|(_, t)| *t).unwrap_or(0);

    if json_out {
        // Build JSON tree
        #[allow(clippy::only_used_in_recursion)]
        fn build_json_tree(
            dir: &Path,
            _root: &Path,
            dir_totals: &HashMap<PathBuf, (u64, usize)>,
            all_files: &[(PathBuf, u64)],
            depth_limit: Option<usize>,
            current_depth: usize,
            budget_remaining: &mut Option<usize>,
        ) -> Value {
            let mut children: Vec<Value> = Vec::new();
            let dir_name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Collect subdirectories
            let mut subdirs: Vec<PathBuf> = dir_totals
                .keys()
                .filter(|p| p.parent().map(|parent| parent == dir).unwrap_or(false) && *p != dir)
                .cloned()
                .collect();
            subdirs.sort();

            // Collect files directly in this dir
            let mut direct_files: Vec<(PathBuf, u64)> = all_files
                .iter()
                .filter(|(fp, _)| fp.parent().map(|parent| parent == dir).unwrap_or(false))
                .cloned()
                .collect();
            direct_files.sort_by(|a, b| a.0.cmp(&b.0));

            for subdir in subdirs {
                if let Some(budget) = budget_remaining.as_mut() {
                    if *budget == 0 {
                        break;
                    }
                }
                let _sub_name = subdir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let (_fc, tt) = dir_totals.get(&subdir).copied().unwrap_or((0, 0));
                let _max_child_depth = depth_limit
                    .map(|max| {
                        if current_depth >= max {
                            0
                        } else {
                            max - current_depth - 1
                        }
                    })
                    .unwrap_or(usize::MAX);

                let child = build_json_tree(
                    &subdir,
                    _root,
                    dir_totals,
                    all_files,
                    depth_limit,
                    current_depth + 1,
                    budget_remaining,
                );

                if let Some(budget) = budget_remaining.as_mut() {
                    *budget = budget.saturating_sub(tt);
                }

                children.push(child);
            }

            for (fp, tokens) in direct_files {
                if let Some(budget) = budget_remaining.as_mut() {
                    if *budget == 0 {
                        break;
                    }
                    *budget = budget.saturating_sub(tokens as usize);
                }
                if depth_limit.map(|max| current_depth >= max).unwrap_or(false) {
                    continue;
                }
                let fname = fp
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                children.push(json!({
                    "name": fname,
                    "type": "file",
                    "tokens": tokens,
                }));
            }

            json!({
                "name": dir_name,
                "type": "dir",
                "children": children,
            })
        }

        let mut budget_remaining = budget;
        let entries = build_json_tree(
            root,
            root,
            &dir_totals,
            &files,
            depth,
            0,
            &mut budget_remaining,
        );

        Ok(json!({
            "command": "map",
            "exit_code": 0,
            "stdout": "",
            "stderr": "",
            "data": {
                "root": root_canonical,
                "total_files": total_files,
                "total_tokens": total_tokens,
                "entries": entries,
            },
            "cache": { "used": false },
        }))
    } else {
        // Text tree
        #[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
        fn build_text_tree(
            dir: &Path,
            _root: &Path,
            dir_totals: &HashMap<PathBuf, (u64, usize)>,
            all_files: &[(PathBuf, u64)],
            prefix: &str,
            depth_limit: Option<usize>,
            current_depth: usize,
            budget_remaining: &mut Option<usize>,
            lines: &mut Vec<String>,
        ) {
            let dir_name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let (total_fc, total_tt) = dir_totals.get(dir).copied().unwrap_or((0, 0));
            lines.push(format!(
                "{}/ ({} files, {} tokens)",
                dir_name, total_fc, total_tt
            ));

            // Collect subdirectories
            let mut subdirs: Vec<PathBuf> = dir_totals
                .keys()
                .filter(|p| p.parent().map(|parent| parent == dir).unwrap_or(false) && *p != dir)
                .cloned()
                .collect();
            subdirs.sort();

            // Collect files directly in this dir
            let mut direct_files: Vec<(PathBuf, u64)> = all_files
                .iter()
                .filter(|(fp, _)| fp.parent().map(|parent| parent == dir).unwrap_or(false))
                .cloned()
                .collect();
            direct_files.sort_by(|a, b| a.0.cmp(&b.0));

            let total_items = subdirs.len() + direct_files.len();

            for (i, subdir) in subdirs.iter().enumerate() {
                if let Some(budget) = budget_remaining.as_mut() {
                    if *budget == 0 {
                        let (_, tt) = dir_totals.get(subdir).copied().unwrap_or((0, 0));
                        if *budget <= tt {
                            lines.push(format!(
                                "{}└── ... (truncated, {} tokens remaining)",
                                prefix, budget
                            ));
                            *budget = 0;
                            return;
                        }
                    }
                }

                let is_last = i == total_items - 1 && direct_files.is_empty();
                let conn = if is_last { "└── " } else { "├── " };
                let child_prefix = if is_last { "    " } else { "│   " };
                let _child_depth = depth_limit
                    .map(|max| {
                        if current_depth >= max {
                            0
                        } else {
                            max - current_depth - 1
                        }
                    })
                    .unwrap_or(usize::MAX);

                let before_len = lines.len();
                build_text_tree(
                    subdir,
                    _root,
                    dir_totals,
                    all_files,
                    &format!("{}{}", prefix, child_prefix),
                    depth_limit,
                    current_depth + 1,
                    budget_remaining,
                    lines,
                );

                if before_len < lines.len() {
                    // Prepend connector to first line (the dir header that was already added)
                    let (fc, tt) = dir_totals.get(subdir).copied().unwrap_or((0, 0));
                    let header = format!(
                        "{}{}/ ({} files, {} tokens)",
                        conn,
                        subdir
                            .file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        fc,
                        tt
                    );
                    if let Some(last) = lines.last_mut() {
                        if *last
                            == format!(
                                "{}/ ({} files, {} tokens)",
                                subdir
                                    .file_name()
                                    .map(|n| n.to_string_lossy())
                                    .unwrap_or_default(),
                                fc,
                                tt
                            )
                        {
                            // Replace the header we just added
                            *last = header;
                        }
                    }
                } else {
                    // Budget ran out, just add a truncated line
                    let (fc, tt) = dir_totals.get(subdir).copied().unwrap_or((0, 0));
                    lines.push(format!(
                        "{}{}/ ({} files, {} tokens)",
                        conn,
                        subdir
                            .file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        fc,
                        tt
                    ));
                }
            }

            for (j, (fp, tokens)) in direct_files.iter().enumerate() {
                let is_last = subdirs.is_empty() && j == direct_files.len() - 1;
                let conn = if is_last { "└── " } else { "├── " };
                let fname = fp
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                if let Some(budget) = budget_remaining.as_mut() {
                    if *budget == 0 {
                        lines.push(format!("{}└── ... (truncated)", prefix));
                        break;
                    }
                    *budget = budget.saturating_sub(*tokens as usize);
                }
                if depth_limit.map(|max| current_depth >= max).unwrap_or(false) {
                    continue;
                }

                lines.push(format!("{}{} ({} tokens)", conn, fname, tokens));
            }
        }

        let mut budget_remaining = budget;
        let mut lines = Vec::new();
        build_text_tree(
            root,
            root,
            &dir_totals,
            &files,
            "",
            depth,
            0,
            &mut budget_remaining,
            &mut lines,
        );

        let text = lines.join("\n");
        Ok(json!({
            "command": "map",
            "exit_code": 0,
            "stdout": text,
            "stderr": "",
            "data": {
                "root": root_canonical,
                "total_files": total_files,
                "total_tokens": total_tokens,
            },
            "cache": { "used": false },
        }))
    }
}

fn estimate_file_tokens(path: &Path) -> u64 {
    // Same heuristic as Document::compute_stats: chars / 4
    fs::read_to_string(path)
        .map(|content| content.chars().count() as u64 / 4)
        .unwrap_or(0)
}

fn tool_symbol(arguments: &Value) -> Result<Value, JsonRpcError> {
    let query: String = parse_arg(arguments, "query")?;
    let file: Option<String> = arguments
        .get("file")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let scope: Option<String> = arguments
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expand: bool = arguments
        .get("expand")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if file.is_some() && scope.is_some() {
        return Err(tool_error(
            -32602,
            "'file' and 'scope' are mutually exclusive",
            None,
        ));
    }

    let re = regex::Regex::new(&query)
        .map_err(|e| tool_error(-32602, &format!("invalid regex: {e}"), None))?;

    let mut results = Vec::new();
    let max_results = 100;

    if let Some(file_path) = file {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(tool_error(
                -32001,
                &format!("file not found: {file_path}"),
                None,
            ));
        }
        search_file(path, &re, expand, &mut results, max_results);
    } else {
        let scope_dir = scope.unwrap_or_else(|| ".".to_string());
        let root = Path::new(&scope_dir);
        if !root.is_dir() {
            return Err(tool_error(
                -32001,
                &format!("not a directory: {scope_dir}"),
                None,
            ));
        }

        let extensions = [
            ".rs", ".py", ".js", ".ts", ".go", ".rb", ".java", ".c", ".cpp", ".h", ".hpp", ".cs",
            ".swift",
        ];
        let walk = walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                e.file_name()
                    .to_str()
                    .map(|s| !s.starts_with('.'))
                    .unwrap_or(false)
            });

        for entry in walk {
            if results.len() >= max_results {
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !extensions.contains(&ext) {
                continue;
            }
            search_file(path, &re, expand, &mut results, max_results);
        }
    }

    let truncated = results.len() >= max_results;
    Ok(json!({
        "command": "symbol",
        "exit_code": 0,
        "stdout": "",
        "stderr": "",
        "data": {
            "symbol": query,
            "results": results,
            "total": results.len(),
            "truncated": truncated,
        },
        "cache": { "used": false },
    }))
}

fn search_file(path: &Path, re: &regex::Regex, expand: bool, results: &mut Vec<Value>, max: usize) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let path_str = path.to_string_lossy().to_string();
    for (i, line) in content.lines().enumerate() {
        if results.len() >= max {
            break;
        }
        if re.is_match(line) {
            let trimmed = line.trim();
            results.push(json!({
                "file": path_str,
                "line": i + 1,
                "content": trimmed,
                "snippet": if expand { trimmed } else { "" },
            }));
        }
    }
}

fn tool_callees(arguments: &Value) -> Result<Value, JsonRpcError> {
    let target: String = parse_arg(arguments, "target")?;
    let scope: String = arguments
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ".".to_string());
    let depth: usize = arguments
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize)
        .unwrap_or(3);
    let json_out: bool = arguments
        .get("json")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if depth == 0 {
        return Err(tool_error(-32602, "depth must be at least 1", None));
    }

    let root = Path::new(&scope);
    if !root.is_dir() {
        return Err(tool_error(
            -32001,
            &format!("not a directory: {scope}"),
            None,
        ));
    }

    // Find all source files
    let extensions = [
        ".rs", ".py", ".js", ".ts", ".go", ".rb", ".java", ".c", ".cpp", ".h", ".hpp", ".cs",
        ".swift",
    ];
    let mut source_files = Vec::new();
    let walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|s| !s.starts_with('.'))
                .unwrap_or(false)
        });

    for entry in walk {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extensions.contains(&ext) {
            source_files.push(path);
        }
    }

    // BFS over function call graph
    let mut visited = HashSet::new();
    let mut results: Vec<Value> = Vec::new();
    let max_results = 100;

    // Find all functions defined in source files (for identifying caller functions)
    // We use a simple heuristic: lines matching `fn ` for Rust, `def ` for Python, etc.
    let mut queue: Vec<(String, usize)> = Vec::new(); // (function_name, current_depth)
    queue.push((target.clone(), 1));

    while let Some((func_name, current_depth)) = queue.pop() {
        if current_depth > depth || results.len() >= max_results {
            continue;
        }

        // Search all source files for call sites: func_name(
        let call_re = match regex::Regex::new(&format!(r"\b{}\s*\(", regex::escape(&func_name))) {
            Ok(r) => r,
            Err(_) => continue,
        };

        for file_path in &source_files {
            let content = match fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let path_str = file_path.to_string_lossy().to_string();

            for (line_no, line) in content.lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }

                if !call_re.is_match(line) {
                    continue;
                }

                // Try to find the containing function definition
                let caller_name = find_containing_function(&content, line_no);

                if let Some(caller) = caller_name {
                    if caller == func_name {
                        // Skip self-references
                        continue;
                    }
                    if visited.contains(&(
                        caller.clone(),
                        func_name.clone(),
                        path_str.clone(),
                        line_no + 1,
                    )) {
                        continue;
                    }
                    visited.insert((
                        caller.clone(),
                        func_name.clone(),
                        path_str.clone(),
                        line_no + 1,
                    ));

                    results.push(json!({
                        "function": caller,
                        "file": path_str.clone(),
                        "line": line_no + 1,
                        "depth": current_depth,
                    }));

                    // Enqueue this caller for the next depth level
                    if current_depth < depth {
                        queue.push((caller, current_depth + 1));
                    }
                }
            }

            if results.len() >= max_results {
                break;
            }
        }
    }

    let truncated = results.len() >= max_results;
    let total = results.len();

    if json_out {
        Ok(json!({
            "command": "callees",
            "exit_code": 0,
            "stdout": "",
            "stderr": "",
            "data": {
                "target": target,
                "depth": depth,
                "results": results,
                "total": total,
                "truncated": truncated,
            },
            "cache": { "used": false },
        }))
    } else {
        // Text output: list
        let mut text_lines = Vec::new();
        text_lines.push(format!("Callees of '{}' (depth {}):", target, depth));
        for r in &results {
            let func = r["function"].as_str().unwrap_or("?");
            let file = r["file"].as_str().unwrap_or("?");
            let line = r["line"].as_u64().unwrap_or(0);
            let d = r["depth"].as_u64().unwrap_or(0);
            text_lines.push(format!("  {}:{} - {} (depth {})", file, line, func, d));
        }
        if truncated {
            text_lines.push(format!("... truncated at {} results", max_results));
        } else if total == 0 {
            text_lines.push("  (no results)".to_string());
        }
        let text = text_lines.join("\n");

        Ok(json!({
            "command": "callees",
            "exit_code": 0,
            "stdout": text,
            "stderr": "",
            "data": {
                "target": target,
                "depth": depth,
                "results": results,
                "total": total,
                "truncated": truncated,
            },
            "cache": { "used": false },
        }))
    }
}

/// Find the name of the function definition that contains the given line number.
/// Uses simple regex matching for common languages.
fn find_containing_function(content: &str, line_no: usize) -> Option<String> {
    // Scan backward from the current line to find the nearest function definition
    let lines: Vec<&str> = content.lines().collect();

    // Use a combined regex to match function defs across languages
    let func_re = regex::Regex::new(
        r"(?:fn\s+(\w+)|def\s+(\w+)|function\s+(\w+)|(\w+)\s*=\s*(?:function|\(|async)|func\s+(\w+)|sub\s+(\w+)|def\w+\s+(\w+))"
    ).ok()?;

    for i in (0..=line_no.min(lines.len().saturating_sub(1))).rev() {
        let line = lines[i];

        if let Some(caps) = func_re.captures(line) {
            let name = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(5))
                .or_else(|| caps.get(6))
                .or_else(|| caps.get(7));
            if let Some(name) = name {
                return Some(name.as_str().to_string());
            }
        }
    }

    None
}
pub fn parse_arg<T: DeserializeOwned>(arguments: &Value, key: &str) -> Result<T, JsonRpcError> {
    let value = arguments.get(key).ok_or_else(|| {
        tool_error(
            -32602,
            &format!("missing required argument '{key}'"),
            Some(json!({ "arguments": arguments })),
        )
    })?;
    serde_json::from_value(value.clone()).map_err(|error| {
        tool_error(
            -32602,
            &format!("invalid argument '{key}': {error}"),
            Some(json!({ "arguments": arguments })),
        )
    })
}

pub fn parse_args<T: DeserializeOwned>(arguments: &Value) -> Result<T, JsonRpcError> {
    serde_json::from_value(arguments.clone()).map_err(|error| {
        tool_error(
            -32602,
            &format!("invalid arguments: {error}"),
            Some(json!({ "arguments": arguments })),
        )
    })
}

pub fn invoke_command(command: Commands) -> Result<(Value, Option<Document>), JsonRpcError> {
    let risk = assess_command(&command);
    let command_name = command_name(&command);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let (exit_code, modified_doc) =
        run_command(command, &mut stdout, &mut stderr).map_err(command_error)?;
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

    Ok((payload, modified_doc))
}

pub fn success_payload(
    command: &str,
    exit_code: i32,
    data: Value,
    cache_stats: &crate::session_cache::CacheStats,
) -> Value {
    json!({
        "command": command,
        "exit_code": exit_code,
        "stdout": "",
        "stderr": "",
        "data": data,
        "cache": {
            "used": cache_stats.hits > 0 || cache_stats.entries > 0,
            "hits": cache_stats.hits,
            "entries": cache_stats.entries,
        },
    })
}

pub fn command_error(error: HashlineError) -> JsonRpcError {
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

pub(crate) fn write_error(
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

pub fn tool_error(code: i32, message: &str, data: Option<Value>) -> JsonRpcError {
    JsonRpcError {
        code,
        message: message.to_owned(),
        data,
    }
}

fn tool_from_diff(arguments: &Value, _session: &mut SessionCache) -> Result<Value, JsonRpcError> {
    let file: String = parse_arg(arguments, "file")?;
    let diff: String = parse_arg(arguments, "diff")?;

    let diff_content = std::fs::read_to_string(&diff)
        .map_err(|e| tool_error(-32603, &format!("cannot read diff file: {e}"), None))?;

    // Simple unified diff parser
    let mut ops: Vec<Value> = Vec::new();

    for line in diff_content.lines() {
        if line.starts_with("@@") {
            // Just record that we saw a hunk header — basic parsing
            ops.push(json!({
                "op": "info",
                "hunk_header": line,
            }));
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if line.starts_with('+') {
            ops.push(json!({
                "op": "insert",
                "content": line.strip_prefix('+').unwrap_or(line),
            }));
        } else if line.starts_with('-') {
            ops.push(json!({
                "op": "delete",
                "content": line.strip_prefix('-').unwrap_or(line),
            }));
        }
    }

    Ok(json!({
        "command": "from_diff",
        "exit_code": 0,
        "stdout": "",
        "stderr": "",
        "data": {
            "file": file,
            "diff_file": diff,
            "operations": ops,
        },
        "cache": { "used": false },
    }))
}

fn tool_merge_patches(
    arguments: &Value,
    _session: &mut SessionCache,
) -> Result<Value, JsonRpcError> {
    let patch_a: String = parse_arg(arguments, "patch_a")?;
    let patch_b: String = parse_arg(arguments, "patch_b")?;
    let base: String = parse_arg(arguments, "base")?;

    let ops_a: Vec<Value> = std::fs::read_to_string(&patch_a)
        .map_err(|e| tool_error(-32603, &format!("cannot read patch_a: {e}"), None))
        .and_then(|text| {
            serde_json::from_str(&text)
                .map_err(|e| tool_error(-32603, &format!("invalid JSON in patch_a: {e}"), None))
        })
        .unwrap_or_default();
    let ops_b: Vec<Value> = std::fs::read_to_string(&patch_b)
        .map_err(|e| tool_error(-32603, &format!("cannot read patch_b: {e}"), None))
        .and_then(|text| {
            serde_json::from_str(&text)
                .map_err(|e| tool_error(-32603, &format!("invalid JSON in patch_b: {e}"), None))
        })
        .unwrap_or_default();

    // Simple merge: detect conflicting anchors
    let mut conflicts: Vec<Value> = Vec::new();
    let anchors_a: std::collections::HashSet<String> = ops_a
        .iter()
        .filter_map(|op| {
            op.get("anchor")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    let mut merged: Vec<Value> = ops_a.clone();

    for op in &ops_b {
        let anchor = op
            .get("anchor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        match anchor {
            Some(ref a) if anchors_a.contains(a) => {
                conflicts.push(json!({
                    "anchor": a,
                    "op_a": null,
                    "op_b": op,
                }));
            }
            _ => {
                merged.push(op.clone());
            }
        }
    }

    Ok(json!({
        "command": "merge_patches",
        "exit_code": if conflicts.is_empty() { 0 } else { 1 },
        "stdout": "",
        "stderr": "",
        "data": {
            "base": base,
            "merged_ops": merged,
            "conflicts": conflicts,
        },
        "cache": { "used": false },
    }))
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "hashline_read",
            "Read a file with current line:hash anchors. For large or noisy files, prefer hashline_index, hashline_grep, or hashline_annotate first; then pass anchor with a small context for snippet-only inspection instead of a full-file read.",
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
            "hashline_index",
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
            "hashline_grep",
            "Search file content and return matching lines with anchors. Prefer this before hashline_read when you know a pattern and need to localize the target without dumping the whole file.",
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
            "hashline_annotate",
            "Map text or regex matches back to current anchors. Prefer this before hashline_read when you know the target text and want a precise anchor for snippet inspection or mutation.",
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
            "hashline_verify",
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
            "hashline_edit",
            "Replace a single line or range at the specified anchor. Use this once anchors are known instead of describing a diff or doing more exploratory reads.",
            mutation_schema("anchor"),
        ),
        tool(
            "hashline_insert",
            "Insert content before or after an anchor. Use this once the insertion anchor is known instead of planning the change in prose.",
            json!({
                "type": "object",
                "properties": mutation_properties("anchor", true),
                "required": ["file", "anchor", "content"]
            }),
        ),
        tool(
            "hashline_delete",
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
            "hashline_patch",
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
            "hashline_swap",
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
            "hashline_move",
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
            "hashline_indent",
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
            "hashline_watch_capabilities",
            "Explain the current watch capability split: the CLI supports continuous watch, while MCP supports only single-event watch calls today.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool(
            "hashline_find_block",
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
            "hashline_stats",
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
            "hashline_symbol",
            "Search for symbol definitions and usages.",
            json!({
                "type": "object",
                "properties": {
                    "query": string_schema("Symbol name to search for."),
                    "file": string_schema("File to search in (mutually exclusive with scope)."),
                    "scope": string_schema("Directory to search in."),
                    "expand": bool_schema("Include source snippets.")
                },
                "required": ["query"]
            }),
        ),
        tool(
            "hashline_doctor",
            "Recommend the safest hashline workflow for a file.",
            json!({
                "type": "object",
                "properties": {
                    "file": string_schema("Target file path.")
                },
                "required": ["file"]
            }),
        ),
        tool(
            "hashline_from_diff",
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
            "hashline_merge_patches",
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
            "hashline_watch",
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
            "hashline_explode",
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
            "hashline_implode",
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
        tool(
            "hashline_map",
            "Map directory tree with estimated token counts. Useful for understanding codebase structure and size.",
            json!({
                "type": "object",
                "properties": {
                    "scope": string_schema("Root directory to map. Defaults to current directory."),
                    "depth": integer_schema("Maximum directory depth to traverse."),
                    "budget": integer_schema("Maximum total tokens before truncation."),
                    "json": bool_schema("Output in JSON format.")
                }
            }),
        ),
        tool(
            "hashline_callees",
            "Find functions called by a given symbol using BFS call graph traversal.",
            json!({
                "type": "object",
                "properties": {
                    "target": string_schema("Symbol name to find callees of."),
                    "scope": string_schema("Directory to search within. Defaults to current directory."),
                    "depth": integer_schema("Maximum BFS depth to traverse. Default: 3."),
                    "json": bool_schema("Output in JSON format.")
                },
                "required": ["target"]
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
    use super::{SERVER_INSTRUCTIONS, SessionCache, dispatch_tool, tool_definitions};
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

    fn json_str(value: &Value) -> Result<&str> {
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
            "hashline_read",
            &json!({
                "file": path,
            }),
            &mut SessionCache::new(128),
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
        let mut session = SessionCache::new(128);

        let read = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let anchor = line_anchor(&read, 2)?;

        let snippet = dispatch_tool(
            "hashline_read",
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
            .find(|tool| tool["name"] == "hashline_read")
            .ok_or_else(|| anyhow!("missing hashline_read tool"))?;
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hashline_edit")
            .ok_or_else(|| anyhow!("missing hashline_edit tool"))?;
        let delete = tools
            .iter()
            .find(|tool| tool["name"] == "hashline_delete")
            .ok_or_else(|| anyhow!("missing hashline_delete tool"))?;

        assert!(
            read["description"]
                .as_str()
                .is_some_and(|text| text.contains("prefer hashline_index"))
        );
        assert!(
            edit["description"]
                .as_str()
                .is_some_and(|text| text.contains("once anchors are known"))
        );
        assert!(delete["description"].as_str().is_some_and(|text| {
            text.contains("instead of leaving deletion as a suggested diff")
        }));
        assert!(SERVER_INSTRUCTIONS.contains("do not start with a full-file hashline_read"));
        assert!(
            SERVER_INSTRUCTIONS
                .contains("Use hashline_edit, hashline_insert, hashline_delete, or hashline_patch")
        );
        Ok(())
    }

    #[test]
    fn read_tool_cache_refreshes_after_file_change() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\n")?;
        let mut session = SessionCache::new(128);

        let first = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        assert_eq!(first["data"]["lines"][0]["content"], "alpha");

        write_text(&path, "beta\n")?;

        let second = must(dispatch_tool(
            "hashline_read",
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
        let mut session = SessionCache::new(128);

        let read = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;

        let result = dispatch_tool(
            "hashline_edit",
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
        let mut session = SessionCache::new(128);

        let read = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;

        let result = dispatch_tool(
            "hashline_delete",
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
                .is_some_and(|text| text.contains("permanently"))
        );
        assert_eq!(read_text(&path)?, "alpha\ndelta\n");
        Ok(())
    }

    #[test]
    fn edit_tool_reports_range_hint_for_dash_separated_qualified_range() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\ndelta\n")?;
        let mut session = SessionCache::new(128);

        let read = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let start = line_anchor(&read, 1)?;
        let end = line_anchor(&read, 2)?;
        let dashed_range = format!("{start}-{end}");

        let error = must_err(dispatch_tool(
            "hashline_edit",
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
    fn grep_tool_returns_matching_lines() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\nbaz hello\n")?;

        let result = dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "hello",
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        assert_eq!(result["command"], "grep");
        assert_eq!(result["exit_code"], 0);
        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["content"], "hello world");
        assert_eq!(lines[1]["content"], "baz hello");
        Ok(())
    }

    #[test]
    fn grep_tool_supports_case_insensitive() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "Hello World\nfoo bar\nbaz hello\n")?;

        let result = dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "hello",
                "case_insensitive": true,
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["content"], "Hello World");
        assert_eq!(lines[1]["content"], "baz hello");
        Ok(())
    }

    #[test]
    fn grep_tool_supports_invert() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\nbaz hello\n")?;

        let result = dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "hello",
                "invert": true,
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        // Two non-matching lines: "foo bar" and the trailing empty line
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["content"], "foo bar");
        assert_eq!(lines[1]["content"], "");
        Ok(())
    }

    #[test]
    fn grep_tool_returns_empty_for_no_match() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\n")?;

        let result = dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "zzzz",
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 0);
        Ok(())
    }

    #[test]
    fn annotate_tool_returns_matching_lines() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\nbaz hello\n")?;

        let result = dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "hello",
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        assert_eq!(result["command"], "annotate");
        assert_eq!(result["exit_code"], 0);
        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["content"], "hello world");
        assert_eq!(lines[1]["content"], "baz hello");
        Ok(())
    }

    #[test]
    fn annotate_tool_supports_regex() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\nbaz hello\n")?;

        let result = dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "h[a-z]+",
                "regex": true,
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["content"], "hello world");
        assert_eq!(lines[1]["content"], "baz hello");
        Ok(())
    }

    #[test]
    fn annotate_tool_regex_case_sensitive_by_default() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "Hello World\nfoo bar\nhello world\n")?;

        let result = dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "hello",
                "regex": true,
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["content"], "hello world");
        Ok(())
    }

    #[test]
    fn annotate_tool_expect_one_ok() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\n")?;

        let result = dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "hello",
                "expect_one": true,
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["content"], "hello world");
        Ok(())
    }

    #[test]
    fn annotate_tool_expect_one_error_on_zero() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\n")?;

        let error = must_err(dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "zzzz",
                "expect_one": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert!(error.message.contains("found 0"));
        Ok(())
    }

    #[test]
    fn annotate_tool_expect_one_error_on_multiple() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\nbaz hello\n")?;

        let error = must_err(dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "hello",
                "expect_one": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert!(error.message.contains("found 2"));
        Ok(())
    }

    #[test]
    fn annotate_tool_returns_empty_for_no_match() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello world\nfoo bar\n")?;

        let result = dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "zzzz",
            }),
            &mut SessionCache::new(128),
        );
        let result = must(result)?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 0);
        Ok(())
    }

    #[test]
    fn annotate_tool_returns_line_numbers_and_hashes() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello\nworld\n")?;

        let result = must(dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "hello",
            }),
            &mut SessionCache::new(128),
        ))?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["n"], 1);
        assert_eq!(lines[0]["hash"].as_str().map(|s| s.len()), Some(2));
        Ok(())
    }

    #[test]
    fn annotate_tool_reports_invalid_regex() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello\n")?;

        let error = must_err(dispatch_tool(
            "hashline_annotate",
            &json!({
                "file": path,
                "query": "(unclosed",
                "regex": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert!(error.message.contains("invalid query"));
        Ok(())
    }

    #[test]
    fn grep_tool_returns_line_numbers_and_hashes() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello\nworld\n")?;

        let result = must(dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "hello",
            }),
            &mut SessionCache::new(128),
        ))?;

        let lines = json_array(&result["data"])?;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["n"], 1);
        assert_eq!(lines[0]["hash"].as_str().map(|s| s.len()), Some(2));
        Ok(())
    }

    #[test]
    fn grep_tool_reports_invalid_pattern() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "hello\n")?;

        let error = must_err(dispatch_tool(
            "hashline_grep",
            &json!({
                "file": path,
                "pattern": "(unclosed",
            }),
            &mut SessionCache::new(128),
        ))?;

        assert!(error.message.contains("invalid pattern"));
        Ok(())
    }

    #[test]
    fn stale_anchor_error_includes_blocked_risk_context() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "alpha\nbeta\ngamma\n")?;
        let mut session = SessionCache::new(128);

        let read = must(dispatch_tool(
            "hashline_read",
            &json!({ "file": path }),
            &mut session,
        ))?;
        let anchor = line_anchor(&read, 1)?;
        // Replace the anchored line content with something whose hash
        // is virtually guaranteed not to collide with `beta`'s hash.
        // (Just reordering would let fuzzy relocation succeed.)
        write_text(&path, "alpha\nDELTA-NEW-CONTENT-XYZ\ngamma\n")?;

        let error = must_err(dispatch_tool(
            "hashline_edit",
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
    fn explode_tool_creates_line_files_and_meta() -> Result<()> {
        let dir = must(TempDir::new())?;
        let src = dir.path().join("src.txt");
        write_text(&src, "alpha\nbeta\ngamma\n")?;
        let out = dir.path().join("exploded");

        let result = must(dispatch_tool(
            "hashline_explode",
            &json!({
                "file": src,
                "out": out,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(result["exit_code"], 0);
        assert!(out.join("L1").exists());
        assert!(out.join("L2").exists());
        assert!(out.join("L3").exists());
        assert!(out.join(".meta.json").exists());

        assert_eq!(read_text(&out.join("L1"))?, "alpha");
        assert_eq!(read_text(&out.join("L2"))?, "beta");
        assert_eq!(read_text(&out.join("L3"))?, "gamma");

        let meta: serde_json::Value = serde_json::from_str(&read_text(&out.join(".meta.json"))?)?;
        assert_eq!(meta["line_count"], 3);
        assert_eq!(meta["newline_style"], "lf");
        assert_eq!(meta["trailing_newline"], true);
        assert_eq!(meta["original"].as_str(), Some(src.to_str().unwrap()));
        Ok(())
    }

    #[test]
    fn explode_tool_rejects_existing_dir_without_force() -> Result<()> {
        let dir = must(TempDir::new())?;
        let src = dir.path().join("src.txt");
        write_text(&src, "content\n")?;
        let out = dir.path().join("exploded");
        must(std::fs::create_dir(&out))?;

        let error = must_err(dispatch_tool(
            "hashline_explode",
            &json!({
                "file": src,
                "out": out,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(error.code, -32001);
        assert!(error.message.contains("already exists"));
        Ok(())
    }

    #[test]
    fn explode_tool_force_overwrites_existing_dir() -> Result<()> {
        let dir = must(TempDir::new())?;
        let src = dir.path().join("src.txt");
        write_text(&src, "content\n")?;
        let out = dir.path().join("exploded");
        must(std::fs::create_dir(&out))?;

        let result = must(dispatch_tool(
            "hashline_explode",
            &json!({
                "file": src,
                "out": out,
                "force": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(result["exit_code"], 0);
        assert!(out.join("L1").exists());
        Ok(())
    }

    #[test]
    fn implode_tool_reassembles_exploded_file() -> Result<()> {
        let dir = must(TempDir::new())?;
        let src = dir.path().join("src.txt");
        write_text(&src, "alpha\nbeta\ngamma\n")?;
        let exploded = dir.path().join("exploded");
        let result = must(dispatch_tool(
            "hashline_explode",
            &json!({
                "file": src,
                "out": exploded,
            }),
            &mut SessionCache::new(128),
        ))?;
        assert_eq!(result["exit_code"], 0);

        let restored = dir.path().join("restored.txt");
        let result = must(dispatch_tool(
            "hashline_implode",
            &json!({
                "dir": exploded,
                "out": restored,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(result["exit_code"], 0);
        assert_eq!(read_text(&restored)?, "alpha\nbeta\ngamma\n");
        Ok(())
    }

    #[test]
    fn implode_tool_rejects_missing_meta() -> Result<()> {
        let dir = must(TempDir::new())?;
        let exploded = dir.path().join("exploded");
        must(std::fs::create_dir(&exploded))?;
        must(std::fs::write(exploded.join("L1"), "hello"))?;

        let error = must_err(dispatch_tool(
            "hashline_implode",
            &json!({
                "dir": exploded,
                "out": dir.path().join("out.txt"),
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(error.code, -32001);
        assert!(error.message.contains("missing .meta.json"));
        Ok(())
    }

    #[test]
    fn implode_tool_rejects_dirty_directory() -> Result<()> {
        let dir = must(TempDir::new())?;
        let exploded = dir.path().join("exploded");
        must(std::fs::create_dir(&exploded))?;
        write_text(
            &exploded.join(".meta.json"),
            "{\"line_count\":1,\"newline_style\":\"lf\",\"trailing_newline\":true}\n",
        )?;
        must(std::fs::write(exploded.join("L1"), "hello"))?;
        must(std::fs::write(exploded.join("notes.txt"), "unexpected"))?;

        let error = must_err(dispatch_tool(
            "hashline_implode",
            &json!({
                "dir": exploded,
                "out": dir.path().join("out.txt"),
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(error.code, -32001);
        Ok(())
    }

    #[test]
    fn watch_capabilities_tool_returns_text() -> Result<()> {
        let result = must(dispatch_tool(
            "hashline_watch_capabilities",
            &json!({}),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(result["exit_code"], 0);
        let text = result["watch_capabilities"]
            .as_str()
            .ok_or_else(|| anyhow!("expected string"))?;
        assert!(text.contains("CLI"));
        assert!(text.contains("MCP"));
        Ok(())
    }

    #[test]
    fn watch_tool_rejects_continuous() -> Result<()> {
        let dir = must(TempDir::new())?;
        let path = dir.path().join("demo.txt");
        write_text(&path, "content\n")?;

        let error = must_err(dispatch_tool(
            "hashline_watch",
            &json!({
                "file": path,
                "continuous": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(error.code, -32602);
        assert!(error.message.contains("continuous mode"));
        Ok(())
    }

    #[test]
    fn implode_tool_dry_run_does_not_write_output() -> Result<()> {
        let dir = must(TempDir::new())?;
        let src = dir.path().join("src.txt");
        write_text(&src, "alpha\nbeta\n")?;
        let exploded = dir.path().join("exploded");
        must(dispatch_tool(
            "hashline_explode",
            &json!({ "file": src, "out": exploded }),
            &mut SessionCache::new(128),
        ))?;

        let restored = dir.path().join("restored.txt");
        let result = must(dispatch_tool(
            "hashline_implode",
            &json!({
                "dir": exploded,
                "out": restored,
                "dry_run": true,
            }),
            &mut SessionCache::new(128),
        ))?;

        assert_eq!(result["exit_code"], 0);
        assert!(!restored.exists());
        Ok(())
    }
}
