// Binary entrypoint. All implementation lives in the `hashline`
// library crate (this same package). Modules previously declared
// here as `mod foo;` are now `pub mod foo;` in `lib.rs` so both
// the bin and external library consumers can reach them.
//
// Items consumed below come from `hashline::*`.

use std::io::{self, BufRead, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use clap::Parser;
use serde_json::Value;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriter;

use hashline::cli::{Cli, Commands};
use hashline::context::{CommandContext, json_pretty_for, output_mode_for};
use hashline::error::HashlineError;
use hashline::orchestration::command_name;

fn main() {
    let cli = Cli::parse();
    init_tracing();
    debug!(command = command_name(&cli.command), "parsed CLI arguments");
    info!(command = command_name(&cli.command), "command started");

    // Check HASHLINE_SOCKET / HASHLINE_URL for daemon routing (skip for
    // serve, mcp, guide, and update commands to avoid routing loops,
    // unsupported tools, and remote self-updates)
    let should_route = !matches!(
        &cli.command,
        Commands::Serve(_) | Commands::Mcp(_) | Commands::Guide(_) | Commands::Update(_)
    );

    // Resolve the daemon tool call up front. `None` means the command
    // uses flags the daemon cannot honor — run it locally rather than
    // silently dropping them (a dropped `--safe` would mean non-atomic
    // writes; a dropped `--json` would corrupt machine-readable output).
    let route = if should_route {
        daemon_route(&cli.command)
    } else {
        None
    };

    if let Some((tool_name, arguments)) = route {
        let no_fallback = hashline::config::env_flag("HASHLINE_NO_FALLBACK");
        // Try HASHLINE_SOCKET first (Unix only)
        #[cfg(unix)]
        if let Some(socket_path) = get_socket_env() {
            match route_via_socket(tool_name, &arguments, &socket_path) {
                Ok(exit_code) => {
                    std::process::exit(exit_code);
                }
                Err(RouteError::Connect(e)) => {
                    if no_fallback {
                        eprintln!("hashline daemon error: {e}");
                        std::process::exit(1);
                    }
                    // Connection-level failure only: the daemon never saw
                    // the request, so local execution cannot double-apply
                    // a mutation. Fall through to HASHLINE_URL / local.
                }
                Err(RouteError::Fatal(e)) => {
                    // The request may already have been executed by the
                    // daemon — falling back could run the mutation twice.
                    eprintln!("hashline daemon error: {e}");
                    std::process::exit(1);
                }
            }
        }
        // Try HASHLINE_URL for HTTP daemon
        if let Some(url) = get_url_env() {
            match route_via_http(tool_name, &arguments, &url) {
                Ok(exit_code) => {
                    std::process::exit(exit_code);
                }
                Err(RouteError::Connect(e)) => {
                    if no_fallback {
                        eprintln!("hashline daemon error: {e}");
                        std::process::exit(1);
                    }
                    // Fall through to local execution
                }
                Err(RouteError::Fatal(e)) => {
                    eprintln!("hashline daemon error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    if let Commands::Mcp(cmd) = &cli.command {
        info!("starting MCP server");
        if let Err(error) = hashline::mcp::run(cmd.clone()) {
            error!(%error, "mcp command failed");
            eprintln!("mcp error: {error}");
            std::process::exit(1);
        }
        info!("mcp server exited cleanly");
        return;
    }

    let output_mode = output_mode_for(&cli.command);
    let json_pretty = json_pretty_for(&cli.command);
    // The automatic update notice must know the command before `cli` is
    // moved into `run`.
    let update_notice_allowed = hashline::update::notice_eligible(&cli.command);
    // Wrap stdout/stderr in BufWriter so large outputs (e.g. `read --json`
    // on a 200k-line file) don't take one syscall per write — serde_json
    // emits many small writes per record, and a fresh `io::stdout()` handle
    // is not block-buffered when piped.
    let stdout_lock = io::stdout().lock();
    let mut stdout = io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
    let stderr_lock = io::stderr().lock();
    let mut stderr = io::BufWriter::with_capacity(8 * 1024, stderr_lock);

    let exit_code = match run(cli, &mut stdout, &mut stderr) {
        Ok(code) => {
            info!(exit_code = code, "command completed");
            code
        }
        Err(error) => {
            if error.log_as_error() {
                error!(%error, "command failed");
            } else {
                warn!(%error, "command rejected");
            }
            let mut context = CommandContext::new(&mut stdout, &mut stderr, output_mode)
                .with_json_pretty(json_pretty);
            let _ = hashline::output::write_error(&mut context, &error);
            1
        }
    };

    // `process::exit` skips destructors, so the BufWriters above must be
    // explicitly flushed or buffered output is silently dropped. We ignore
    // flush errors — there is no useful action to take if writing to a
    // closed pipe fails at process exit.
    let _ = stdout.flush();
    let _ = stderr.flush();

    // Best-effort outdated-version notice, after all command output. Gated
    // on a terminal stderr so agent pipelines keep byte-identical output.
    if exit_code == 0 && update_notice_allowed {
        hashline::update::maybe_print_update_notice(io::stderr().is_terminal());
    }

    std::process::exit(exit_code);
}

fn init_tracing() {
    // An invalid HASHLINE_LOG filter warns and falls back to `info`
    // rather than silently disabling all logging.
    let filter = match tracing_filter() {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!("warning: invalid HASHLINE_LOG filter ({error}); using 'info'");
            match EnvFilter::try_new("info") {
                Ok(filter) => filter,
                Err(_) => return,
            }
        }
    };

    let log_path = match resolve_log_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("warning: tracing log path unavailable: {error}");
            return;
        }
    };

    let writer = match SharedFileWriter::new(&log_path) {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!(
                "warning: failed to open hashline log file {}: {error}",
                log_path.display()
            );
            return;
        }
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .compact()
        .try_init();
}

fn tracing_filter() -> Result<EnvFilter, tracing_subscriber::filter::ParseError> {
    match std::env::var("HASHLINE_LOG") {
        Ok(value) if !value.trim().is_empty() => EnvFilter::try_new(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => EnvFilter::try_new("info"),
        Err(std::env::VarError::NotUnicode(_)) => EnvFilter::try_new("info"),
    }
}

fn resolve_log_path() -> Result<PathBuf, String> {
    match std::env::var("HASHLINE_LOG_PATH") {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(default_log_path(hashline_home_dir()?)),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("HASHLINE_LOG_PATH is not valid unicode".into())
        }
    }
}

fn hashline_home_dir() -> Result<PathBuf, String> {
    hashline::config::hashline_home_dir().ok_or_else(|| "home directory not set".into())
}

fn default_log_path(home: PathBuf) -> PathBuf {
    home.join(".hashline").join("hashline.log")
}

/// Default cap on the hashline log file. When the log exceeds this many
/// bytes the writer truncates the file to the most recent half so the
/// process never runs the disk out of space from accumulated log lines.
const DEFAULT_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB

fn log_max_bytes_from_env() -> u64 {
    match std::env::var("HASHLINE_LOG_MAX_BYTES") {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_LOG_MAX_BYTES),
        _ => DEFAULT_LOG_MAX_BYTES,
    }
}

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<std::fs::File>>,
    path: PathBuf,
    bytes_written: Arc<std::sync::atomic::AtomicU64>,
    max_bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl SharedFileWriter {
    fn new(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let initial_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            path: path.to_path_buf(),
            bytes_written: Arc::new(std::sync::atomic::AtomicU64::new(initial_size)),
            max_bytes: Arc::new(std::sync::atomic::AtomicU64::new(log_max_bytes_from_env())),
        })
    }

    /// Truncate the log to retain the most recent `keep` bytes.
    ///
    /// Reads the trailing `keep` bytes (rounded to the next newline so we
    /// never slice a UTF-8 multi-byte sequence) from the current file
    /// and writes the retained tail back via `std::fs::write`. The caller
    /// is responsible for holding the writer mutex.
    fn rotate_locked(&self, _file: &mut std::fs::File, keep: u64, path: &Path) -> io::Result<()> {
        use std::io::Read;
        let len = std::fs::metadata(path)?.len();
        if len <= keep {
            return Ok(());
        }
        let skip = len - keep;
        // Open a dedicated read handle (not the mutex-guarded one) so we
        // never hit EBADF from racing with the MutexGuard's file handle.
        let mut read_file = std::fs::File::open(path)?;
        use std::io::Seek;
        read_file.seek(std::io::SeekFrom::Start(skip))?;
        let mut tail = Vec::with_capacity(keep as usize);
        read_file.read_to_end(&mut tail)?;
        // Find the first newline so we start on a complete log line.
        let start = tail
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        let tail = tail.split_off(start);
        // Overwrite the file via a fresh write-only handle. This avoids
        // any interaction with the MutexGuard's append-mode handle.
        std::fs::write(path, &tail)?;
        self.bytes_written
            .store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

impl Write for SharedFileGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.guard.write(buf)?;
        let new_total = self
            .writer
            .bytes_written
            .fetch_add(written as u64, std::sync::atomic::Ordering::Relaxed)
            + written as u64;
        let cap = self
            .writer
            .max_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        if cap > 0 && new_total > cap {
            let keep = cap / 2;
            if self
                .writer
                .rotate_locked(&mut self.guard, keep, &self.writer.path)
                .is_err()
            {
                // best-effort
            }
            if let Ok(new_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.writer.path)
            {
                let _ = std::mem::replace(&mut *self.guard, new_file);
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.guard.flush()
    }
}

struct SharedFileGuard<'a> {
    guard: MutexGuard<'a, std::fs::File>,
    writer: &'a SharedFileWriter,
}

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = SharedFileGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SharedFileGuard {
            guard: self
                .file
                .lock()
                .expect("hashline tracing file lock poisoned"),
            writer: self,
        }
    }
}

fn run<W: Write, E: Write>(cli: Cli, stdout: &mut W, stderr: &mut E) -> Result<i32, HashlineError> {
    hashline::orchestration::run_command(cli.command, stdout, stderr).map(|(code, _)| code)
}

/// Map a CLI command to the MCP tool name and arguments the daemon
/// expects, or `None` when the command uses flags the daemon cannot
/// honor (in which case the caller runs it locally — never silently
/// drop a flag like `--safe`, which would mean non-atomic writes).
///
/// The MCP tool surface is narrower than the CLI: `patch`/`write` have
/// no `--safe`/`--verbose` (`--safe` would mean non-atomic writes and
/// `--verbose` has no daemon equivalent), `find_block` has no
/// `--verbose`/`--pretty`, `remove`/`rename` are exposed as
/// `remove_file`/`rename_file`, and `rename --force` has no daemon
/// equivalent (the tool always refuses to overwrite). `--json`,
/// `--dry-run`, `--force` (write), and `--emit-anchors` do map onto
/// tool arguments.
fn daemon_route(command: &Commands) -> Option<(&'static str, Value)> {
    match command {
        Commands::Read(cmd) => Some((
            "read",
            serde_json::json!({
                "file": cmd.file.to_string_lossy(),
                "json": cmd.json,
            }),
        )),
        Commands::Patch(cmd) => {
            if cmd.safe || cmd.verbose {
                return None;
            }
            Some((
                "patch",
                serde_json::json!({
                    "file": cmd.file.to_string_lossy(),
                    "patch": cmd.patch.as_str(),
                    "dry_run": cmd.dry_run,
                    "json": cmd.json,
                    // The MCP tool calls this `return_updated_anchors`;
                    // the CLI flag/env var is `emit_anchors`.
                    "return_updated_anchors": cmd.emit_anchors,
                }),
            ))
        }
        Commands::Write(cmd) => {
            if cmd.safe || cmd.verbose {
                return None;
            }
            Some((
                "write",
                serde_json::json!({
                    "file": cmd.file.to_string_lossy(),
                    "content": cmd.content.as_str(),
                    "force": cmd.force,
                    "json": cmd.json,
                }),
            ))
        }
        Commands::FindBlock(cmd) => {
            if cmd.verbose || cmd.pretty {
                return None;
            }
            Some((
                "find_block",
                serde_json::json!({
                    "file": cmd.file.to_string_lossy(),
                    "anchor": cmd.anchor.as_str(),
                    "json": cmd.json,
                }),
            ))
        }
        Commands::Remove(cmd) => {
            if cmd.verbose {
                return None;
            }
            Some((
                "remove_file",
                serde_json::json!({
                    "file": cmd.file.to_string_lossy(),
                    "json": cmd.json,
                }),
            ))
        }
        Commands::Rename(cmd) => {
            if cmd.verbose || cmd.force {
                return None;
            }
            Some((
                "rename_file",
                serde_json::json!({
                    "src": cmd.src.to_string_lossy(),
                    "dst": cmd.dst.to_string_lossy(),
                    "json": cmd.json,
                }),
            ))
        }
        Commands::Guide(_) | Commands::Serve(_) | Commands::Mcp(_) | Commands::Update(_) => None,
    }
}

/// Why a daemon-routed command failed.
///
/// `Connect` means the request never reached the daemon, so falling back
/// to local execution cannot double-apply a mutation. `Fatal` means the
/// request may already have been executed (or the daemon answered with
/// an error) — falling back could run the mutation twice, so the caller
/// must surface the error instead.
enum RouteError {
    Connect(String),
    Fatal(String),
}

/// Timeouts for daemon routing. Without them a wedged daemon hangs every
/// CLI invocation indefinitely (a 21.5s connect stall was verified).
const DAEMON_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const DAEMON_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Get the daemon socket path from the HASHLINE_SOCKET env var.
#[cfg(unix)]
fn get_socket_env() -> Option<PathBuf> {
    std::env::var("HASHLINE_SOCKET").ok().map(PathBuf::from)
}

/// Get the daemon URL from the HASHLINE_URL env var.
fn get_url_env() -> Option<String> {
    std::env::var("HASHLINE_URL").ok()
}

/// Heuristic for daemon text that reports failure without the MCP
/// `isError` flag. Tool handlers prefix errors with `Error:` but at
/// least one path emits `Error writing file: ...` — and older/foreign
/// daemons may not set `isError` at all. Command success output always
/// starts with `OK `, `[`, `{`, or `Dry-run`, so an `Error` prefix is a
/// reliable failure signal.
fn daemon_text_is_error(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("Error:") || t.starts_with("Error ")
}

/// Emit a daemon tool result and return the exit code the CLI should use.
///
/// The daemon answers with the MCP `CallToolResult` shape. A tool failure
/// is flagged with the MCP `isError` field *or* an `Error`-prefixed text
/// payload: its text belongs on stderr and the command must fail.
/// Without this, a routed command reports exit 0 for every error and
/// writes the error text into the machine-readable stdout stream, so a
/// caller sees a failed edit as a successful one.
fn emit_daemon_result(result: &Value) -> i32 {
    let mut is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut text = String::new();
    if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
        for item in items {
            if let Some(chunk) = item.get("text").and_then(|v| v.as_str()) {
                text.push_str(chunk);
            }
        }
    }
    is_error = is_error || daemon_text_is_error(&text);

    if is_error {
        if !text.is_empty() {
            eprint!("{text}");
        }
        1
    } else {
        if !text.is_empty() {
            print!("{text}");
        }
        0
    }
}

/// True when `structuredContent` has the legacy daemon envelope shape
/// (`{stdout, stderr, exit_code, data}`) rather than the current MCP
/// shape, where `structuredContent` is the tool handler's own object
/// and the human-readable output lives in `result.content[].text`.
fn is_legacy_daemon_envelope(content: &Value) -> bool {
    content.is_object()
        && ["stdout", "stderr", "exit_code", "data"]
            .iter()
            .any(|key| content.get(key).is_some())
}

/// Emit the `{stdout, stderr, exit_code, data}` (or legacy MCP `content`)
/// envelope a daemon returns inside `structuredContent`, and return the
/// exit code. Failure is detected via `isError` on either the outer
/// result or the structured content itself, or an `Error`-prefixed
/// payload — older daemons and the HTTP wrapper nest `isError` inside
/// `structuredContent`, which a top-level-only check would miss.
fn emit_structured_daemon_content(content: &Value, outer_is_error: bool) -> i32 {
    let mut is_error = outer_is_error
        || content
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    let stdout_text = content.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let has_stdout = !stdout_text.is_empty();

    // Collect any fallback text up front so the error heuristic can see it.
    let mut fallback_text = String::new();
    if content.get("data").is_none() && !has_stdout {
        if let Some(content_arr) = content.get("content").and_then(|c| c.as_array()) {
            for item in content_arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    fallback_text.push_str(text);
                }
            }
        }
    }
    is_error =
        is_error || daemon_text_is_error(stdout_text) || daemon_text_is_error(&fallback_text);

    if !stdout_text.is_empty() {
        if is_error {
            eprint!("{stdout_text}");
        } else {
            print!("{stdout_text}");
        }
    }
    if let Some(stderr_text) = content.get("stderr").and_then(|v| v.as_str()) {
        if !stderr_text.is_empty() {
            eprint!("{stderr_text}");
        }
    }
    if !has_stdout {
        if let Some(data) = content.get("data") {
            let _ = serde_json::to_writer(std::io::stdout().lock(), data);
            println!();
        } else if !fallback_text.is_empty() {
            // On failure the text is an error message — keep it off the
            // machine-readable stdout stream.
            if is_error {
                eprint!("{fallback_text}");
            } else {
                print!("{fallback_text}");
            }
        }
    }
    let exit_code = content
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    if is_error { 1 } else { exit_code }
}

/// Dispatch a daemon `result` object to the right emitter.
///
/// Two `structuredContent` shapes exist in the wild:
///  - legacy envelope `{stdout, stderr, exit_code, data}` →
///    [`emit_structured_daemon_content`];
///  - current MCP shape, where `structuredContent` is the tool handler's
///    own object and the output text lives in `result.content[].text` →
///    [`emit_daemon_result`]. When neither text nor envelope is present,
///    the structured object itself is printed so the caller is not left
///    with silent empty output.
fn emit_daemon_response(result: Option<&Value>) -> i32 {
    let Some(result) = result else {
        return 0;
    };
    if let Some(sc) = result.get("structuredContent") {
        if is_legacy_daemon_envelope(sc) {
            let outer_is_error = result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            return emit_structured_daemon_content(sc, outer_is_error);
        }
        let has_text = result
            .get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("text")
                        .and_then(|v| v.as_str())
                        .is_some_and(|t| !t.is_empty())
                })
            });
        if !has_text {
            let is_error = result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || sc.get("isError").and_then(|v| v.as_bool()).unwrap_or(false)
                || sc.get("success").and_then(|v| v.as_bool()) == Some(false);
            if let Ok(s) = serde_json::to_string(sc) {
                println!("{s}");
            }
            return if is_error { 1 } else { 0 };
        }
    }
    emit_daemon_result(result)
}

/// Route a CLI command through a Unix socket daemon.
#[cfg(unix)]
fn route_via_socket(
    tool_name: &str,
    arguments: &Value,
    socket_path: &Path,
) -> Result<i32, RouteError> {
    let stream = UnixStream::connect(socket_path).map_err(|e| {
        RouteError::Connect(format!(
            "cannot connect to daemon at {}: {e}",
            socket_path.display()
        ))
    })?;
    // Bound every subsequent read/write so a wedged daemon cannot hang
    // the CLI forever.
    stream
        .set_read_timeout(Some(DAEMON_IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(DAEMON_IO_TIMEOUT)))
        .map_err(|e| RouteError::Fatal(format!("cannot set daemon timeout: {e}")))?;

    let mut reader = io::BufReader::new(
        stream
            .try_clone()
            .map_err(|e| RouteError::Fatal(format!("clone error: {e}")))?,
    );
    let mut writer = &stream;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        },
        "id": 1,
    });

    let request_str = serde_json::to_string(&request)
        .map_err(|e| RouteError::Fatal(format!("json error: {e}")))?;

    writer
        .write_all(request_str.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|e| RouteError::Fatal(format!("write error: {e}")))?;

    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| RouteError::Fatal(format!("read error: {e}")))?;

    let response: Value = serde_json::from_str(&response_line)
        .map_err(|e| RouteError::Fatal(format!("parse response error: {e}")))?;

    if let Some(error) = response.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        // The daemon answered — the tool may or may not have run, but a
        // JSON-RPC error means we cannot trust a local retry either way
        // for mutations. Surface it; never fall back.
        return Err(RouteError::Fatal(msg.to_string()));
    }

    Ok(emit_daemon_response(response.get("result")))
}

/// Parse `HASHLINE_URL` into `(host, port)`.
///
/// Accepts `http://host[:port][/path]`, `https://host[:port][/path]`
/// (dialed as plain HTTP — the daemon has no TLS), `host:port`, or a
/// bare `host`. The default port is 17300. IPv6 literals in brackets
/// are supported. Malformed values are an error, never a silent
/// mis-parse (previously `https://h` produced host `"https"` and
/// `http://h:9999/path` reset the port to 17300).
fn parse_daemon_url(url: &str) -> Result<(String, u16), String> {
    const DEFAULT_PORT: u16 = 17300;
    let trimmed = url.trim();
    let rest = match trimmed.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(format!(
                    "unsupported HASHLINE_URL scheme '{scheme}' (expected http:// or https://)"
                ));
            }
            rest
        }
        None => trimmed,
    };
    // Authority ends at the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(format!("HASHLINE_URL '{url}' has no host"));
    }

    // Bracketed IPv6 literal: [::1] or [::1]:8080
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| format!("unterminated IPv6 literal in HASHLINE_URL '{url}'"))?;
        let host = &authority[1..close];
        let after = &authority[close + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| format!("invalid port in HASHLINE_URL '{url}'"))?,
            None if after.is_empty() => DEFAULT_PORT,
            None => return Err(format!("unexpected text after IPv6 host in '{url}'")),
        };
        return Ok((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port_str)) => {
            if host.is_empty() {
                return Err(format!("missing host in HASHLINE_URL '{url}'"));
            }
            let port = port_str
                .parse::<u16>()
                .map_err(|_| format!("invalid port in HASHLINE_URL '{url}'"))?;
            Ok((host.to_string(), port))
        }
        None => Ok((authority.to_string(), DEFAULT_PORT)),
    }
}

/// Route a CLI command through an HTTP daemon.
fn route_via_http(tool_name: &str, arguments: &Value, url: &str) -> Result<i32, RouteError> {
    // Parse URL to get host and port. A malformed HASHLINE_URL is a
    // configuration problem, not a daemon failure — classify as Connect
    // so the caller may fall back to local execution.
    let (host, port) = parse_daemon_url(url).map_err(RouteError::Connect)?;

    // Resolve and dial with an explicit connect timeout — `TcpStream::
    // connect` on a string address has none and a blackholed daemon
    // address hung the CLI for 20+ seconds.
    use std::net::ToSocketAddrs;
    let addr = format!("{host}:{port}");
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| RouteError::Connect(format!("cannot resolve daemon at {addr}: {e}")))?;
    let mut stream = None;
    let mut last_err = String::new();
    for sock_addr in addrs {
        match std::net::TcpStream::connect_timeout(&sock_addr, DAEMON_CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        RouteError::Connect(format!("cannot connect to daemon at {addr}: {last_err}"))
    })?;
    stream
        .set_read_timeout(Some(DAEMON_IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(DAEMON_IO_TIMEOUT)))
        .map_err(|e| RouteError::Fatal(format!("cannot set daemon timeout: {e}")))?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        },
        "id": 1,
    });

    let body = serde_json::to_string(&request)
        .map_err(|e| RouteError::Fatal(format!("json error: {e}")))?;

    // `serve --http` requires a bearer token (bd-serve-http-unauthenticated-
    // 06tw): send it when the client can resolve one — HASHLINE_TOKEN env
    // or the persisted ~/.hashline/http.token. A daemon that predates auth
    // ignores the header.
    let auth_header = hashline::commands::serve::read_http_token()
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();

    let http_request = format!(
        "POST /rpc HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n{auth_header}Content-Length: {}\r\n\r\n{}",
        body.len(),
        body,
    );

    use std::io::Write;
    stream
        .write_all(http_request.as_bytes())
        .map_err(|e| RouteError::Fatal(format!("write error: {e}")))?;

    let mut reader = io::BufReader::new(&stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| RouteError::Fatal(format!("read error: {e}")))?;

    // Read headers (case-insensitive names)
    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|e| RouteError::Fatal(format!("read header error: {e}")))?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body_buf = vec![0u8; content_length];
    reader
        .read_exact(&mut body_buf)
        .map_err(|e| RouteError::Fatal(format!("read body error: {e}")))?;

    let response: Value = serde_json::from_slice(&body_buf)
        .map_err(|e| RouteError::Fatal(format!("parse response error: {e}")))?;

    if let Some(error) = response.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        return Err(RouteError::Fatal(msg.to_string()));
    }

    Ok(emit_daemon_response(response.get("result")))
}

#[cfg(test)]
mod tests {
    use super::{
        daemon_route, daemon_text_is_error, default_log_path, emit_daemon_response,
        emit_daemon_result, emit_structured_daemon_content, is_legacy_daemon_envelope,
        parse_daemon_url, run, tracing_filter,
    };
    use hashline::cli::{
        Cli, Commands, FindBlockCmd, PatchCmd, ReadCmd, RemoveCmd, RenameCmd, WriteCmd,
    };
    use std::path::PathBuf;

    #[test]
    fn pretty_errors_go_to_stderr_only() {
        let cli = Cli {
            command: Commands::Read(ReadCmd {
                file: PathBuf::from("missing.txt"),
                json: false,
                no_cache: false,
            }),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run(cli, &mut stdout, &mut stderr);
        if let Err(error) = result {
            let mut sink_out = Vec::new();
            let mut sink_err = Vec::new();
            let mut ctx = hashline::context::CommandContext::new(
                &mut sink_out,
                &mut sink_err,
                hashline::context::OutputMode::Verbose,
            );
            hashline::output::write_error(&mut ctx, &error).unwrap();
            stdout = sink_out;
            stderr = sink_err;
        }

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Error: I/O error:"));
        assert!(
            stderr.contains("Hint: check the file path and permissions, then retry the command")
        );
    }

    #[test]
    fn default_log_path_is_under_hashline_home_dir() {
        let path = default_log_path(PathBuf::from("/tmp/test-home"));
        assert_eq!(path, PathBuf::from("/tmp/test-home/.hashline/hashline.log"));
    }

    #[test]
    fn tracing_filter_defaults_to_info() {
        let filter = tracing_filter().unwrap();
        let rendered = filter.to_string();
        assert!(rendered.contains("info"));
    }

    // ---- Daemon routing must preserve failures (bd-bx2) --------------------
    //
    // A command routed through a daemon used to exit 0 for every error and
    // write the error text into stdout, so a caller could not tell a failed
    // edit from a successful one.

    #[test]
    fn daemon_error_result_exits_non_zero() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Error: file not found: missing.txt"}],
            "isError": true,
        });
        assert_eq!(emit_daemon_result(&result), 1);
    }

    #[test]
    fn daemon_success_result_exits_zero() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "OK file.txt#abcd lines=1"}],
        });
        assert_eq!(emit_daemon_result(&result), 0);
    }

    #[test]
    fn daemon_result_without_content_exits_zero() {
        let result = serde_json::json!({});
        assert_eq!(emit_daemon_result(&result), 0);
    }

    #[test]
    fn daemon_error_writing_file_text_exits_non_zero() {
        // `handle_patch` reports write failures as "Error writing file: ..."
        // which does not start with "Error: " — the isError flag may also
        // be absent on older daemons.
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "Error writing file: permission denied"}],
        });
        assert_eq!(emit_daemon_result(&result), 1);
    }

    #[test]
    fn daemon_structured_error_nested_iserror_exits_non_zero() {
        // The HTTP daemon wraps the CallToolResult inside structuredContent;
        // isError lives on the inner object, not the outer result.
        let content = serde_json::json!({
            "content": [{"type": "text", "text": "Error: file not found: x"}],
            "isError": true,
        });
        assert_eq!(emit_structured_daemon_content(&content, false), 1);
    }

    #[test]
    fn daemon_structured_success_exits_zero() {
        let content = serde_json::json!({
            "stdout": "OK file.txt#abcd lines=1\n",
            "stderr": "",
            "exit_code": 0,
        });
        assert_eq!(emit_structured_daemon_content(&content, false), 0);
    }

    // ---- Current MCP result shape (content[].text + structuredContent) --

    #[test]
    fn daemon_response_uses_text_content_for_current_mcp_shape() {
        // mcp.rs tool_result: output text in content[], handler object in
        // structuredContent — NOT the legacy {stdout,...} envelope.
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "OK a.txt#1234 lines=1\n"}],
            "structuredContent": {"success": true, "file": "a.txt", "lines": 1},
        });
        assert_eq!(emit_daemon_response(Some(&result)), 0);
    }

    #[test]
    fn daemon_response_flags_structured_failure_without_text() {
        // No content[] text: the structured object's success=false must
        // still produce a non-zero exit.
        let result = serde_json::json!({
            "structuredContent": {"success": false, "kind": "IO", "error": "boom"},
        });
        assert_eq!(emit_daemon_response(Some(&result)), 1);
    }

    #[test]
    fn daemon_response_none_exits_zero() {
        assert_eq!(emit_daemon_response(None), 0);
    }

    #[test]
    fn legacy_envelope_detection() {
        assert!(is_legacy_daemon_envelope(&serde_json::json!({
            "stdout": "x", "exit_code": 0
        })));
        assert!(!is_legacy_daemon_envelope(&serde_json::json!({
            "success": true, "file": "a.txt"
        })));
        assert!(!is_legacy_daemon_envelope(&serde_json::json!("text")));
    }

    // ---- Daemon routing table (bd-daemon-routing-nzz) -------------------

    #[test]
    fn daemon_route_uses_mcp_tool_names() {
        let remove = Commands::Remove(RemoveCmd {
            file: PathBuf::from("a.txt"),
            json: false,
            verbose: false,
        });
        let (name, _) = daemon_route(&remove).unwrap();
        assert_eq!(name, "remove_file");

        let rename = Commands::Rename(RenameCmd {
            src: PathBuf::from("a.txt"),
            dst: PathBuf::from("b.txt"),
            json: false,
            verbose: false,
            force: false,
        });
        let (name, _) = daemon_route(&rename).unwrap();
        assert_eq!(name, "rename_file");
    }

    #[test]
    fn daemon_route_rejects_unsupported_flags() {
        // --safe must never be silently dropped: it is the difference
        // between atomic and non-atomic writes.
        let patch_safe = Commands::Patch(PatchCmd {
            file: PathBuf::from("a.txt"),
            patch: "DEL 1:ab".into(),
            dry_run: false,
            json: false,
            verbose: false,
            safe: true,
            emit_anchors: false,
        });
        assert!(daemon_route(&patch_safe).is_none());

        // --verbose has no daemon equivalent on any command.
        let patch_verbose = Commands::Patch(PatchCmd {
            verbose: true,
            ..match &patch_safe {
                Commands::Patch(c) => c.clone(),
                _ => unreachable!(),
            }
        });
        assert!(daemon_route(&patch_verbose).is_none());

        let write_safe = Commands::Write(WriteCmd {
            file: PathBuf::from("a.txt"),
            content: "x".into(),
            force: false,
            json: false,
            verbose: false,
            safe: true,
        });
        assert!(daemon_route(&write_safe).is_none());

        let write_verbose = Commands::Write(WriteCmd {
            verbose: true,
            ..match &write_safe {
                Commands::Write(c) => c.clone(),
                _ => unreachable!(),
            }
        });
        assert!(daemon_route(&write_verbose).is_none());

        // rename --force has no daemon equivalent (tool refuses overwrite)
        let rename_force = Commands::Rename(RenameCmd {
            src: PathBuf::from("a.txt"),
            dst: PathBuf::from("b.txt"),
            json: false,
            verbose: false,
            force: true,
        });
        assert!(daemon_route(&rename_force).is_none());

        let find_verbose = Commands::FindBlock(FindBlockCmd {
            file: PathBuf::from("a.txt"),
            anchor: "1:ab".into(),
            json: false,
            verbose: true,
            pretty: false,
        });
        assert!(daemon_route(&find_verbose).is_none());

        let find_pretty = Commands::FindBlock(FindBlockCmd {
            file: PathBuf::from("a.txt"),
            anchor: "1:ab".into(),
            json: true,
            verbose: false,
            pretty: true,
        });
        assert!(daemon_route(&find_pretty).is_none());
    }

    #[test]
    fn daemon_route_maps_emit_anchors_and_passthrough_flags() {
        let patch = Commands::Patch(PatchCmd {
            file: PathBuf::from("a.txt"),
            patch: "DEL 1:ab".into(),
            dry_run: true,
            json: false,
            verbose: false,
            safe: false,
            emit_anchors: true,
        });
        let (name, args) = daemon_route(&patch).unwrap();
        assert_eq!(name, "patch");
        assert_eq!(args["dry_run"], true);
        assert_eq!(args["return_updated_anchors"], true);
        assert_eq!(args["file"], "a.txt");

        // --json maps onto the tool's json argument on patch/write/read.
        let patch_json = Commands::Patch(PatchCmd {
            json: true,
            ..match &patch {
                Commands::Patch(c) => c.clone(),
                _ => unreachable!(),
            }
        });
        let (_, args) = daemon_route(&patch_json).unwrap();
        assert_eq!(args["json"], true);

        let write = Commands::Write(WriteCmd {
            file: PathBuf::from("a.txt"),
            content: "hi".into(),
            force: true,
            json: true,
            verbose: false,
            safe: false,
        });
        let (name, args) = daemon_route(&write).unwrap();
        assert_eq!(name, "write");
        assert_eq!(args["force"], true);
        assert_eq!(args["json"], true);
    }

    #[test]
    fn daemon_route_rejects_non_tool_commands() {
        // Guide must never reach the router: it previously hit
        // unreachable!() and panicked when a daemon was reachable.
        let guide = Commands::Guide(hashline::cli::GuideCmd { json: false });
        assert!(daemon_route(&guide).is_none());
    }

    // ---- HASHLINE_URL parsing (bd-env-vars-home-4j8) --------------------

    #[test]
    fn parse_daemon_url_handles_schemes_and_paths() {
        assert_eq!(
            parse_daemon_url("http://127.0.0.1:17300").unwrap(),
            ("127.0.0.1".to_string(), 17300)
        );
        // https scheme must not leak into the host
        assert_eq!(
            parse_daemon_url("https://daemon.local:9999/path").unwrap(),
            ("daemon.local".to_string(), 9999)
        );
        // path must not reset the port
        assert_eq!(
            parse_daemon_url("http://host:9999/path").unwrap(),
            ("host".to_string(), 9999)
        );
        // bare host:port and bare host
        assert_eq!(
            parse_daemon_url("host:1234").unwrap(),
            ("host".to_string(), 1234)
        );
        assert_eq!(
            parse_daemon_url("host").unwrap(),
            ("host".to_string(), 17300)
        );
        // IPv6
        assert_eq!(
            parse_daemon_url("http://[::1]:8080").unwrap(),
            ("::1".to_string(), 8080)
        );
    }

    #[test]
    fn parse_daemon_url_rejects_malformed() {
        assert!(parse_daemon_url("https://").is_err());
        assert!(parse_daemon_url("ftp://host").is_err());
        assert!(parse_daemon_url("http://host:notaport").is_err());
        assert!(parse_daemon_url("").is_err());
    }

    #[test]
    fn daemon_text_is_error_detects_error_prefixes() {
        assert!(daemon_text_is_error("Error: file not found"));
        assert!(daemon_text_is_error("Error writing file: denied"));
        assert!(!daemon_text_is_error("OK file.txt#abcd lines=1"));
        assert!(!daemon_text_is_error("[file.txt#ab12]"));
        assert!(!daemon_text_is_error(""));
    }
}
