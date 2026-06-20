// Binary entrypoint. All implementation lives in the `hashline`
// library crate (this same package). Modules previously declared
// here as `mod foo;` are now `pub mod foo;` in `lib.rs` so both
// the bin and external library consumers can reach them.
//
// Items consumed below come from `hashline::*`.

use std::io::{self, BufRead, Read, Write};
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

    // Check HASHLINE_SOCKET for daemon routing (skip for serve and mcp commands
    // to avoid routing loops)
    let should_route = !matches!(&cli.command, Commands::Serve(_) | Commands::Mcp(_));

    if should_route {
        let no_fallback = std::env::var("HASHLINE_NO_FALLBACK").is_ok();
        // Try HASHLINE_SOCKET first (Unix only)
        #[cfg(unix)]
        if let Some(socket_path) = get_socket_env() {
            match route_via_socket(&cli, &socket_path) {
                Ok(exit_code) => {
                    std::process::exit(exit_code);
                }
                Err(e) if no_fallback => {
                    eprintln!("hashline daemon error: {e}");
                    std::process::exit(1);
                }
                Err(_) => {
                    // Fall through to local execution
                }
            }
        }
        // Try HASHLINE_URL for HTTP daemon
        if let Some(url) = get_url_env() {
            match route_via_http(&cli, &url) {
                Ok(exit_code) => {
                    std::process::exit(exit_code);
                }
                Err(e) if no_fallback => {
                    eprintln!("hashline daemon error: {e}");
                    std::process::exit(1);
                }
                Err(_) => {
                    // Fall through to local execution
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

    std::process::exit(exit_code);
}

fn init_tracing() {
    let filter = match tracing_filter() {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!("warning: invalid HASHLINE_LOG filter: {error}");
            return;
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
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .map_err(|_| "USERPROFILE not set".into())
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| "HOME not set".into())
    }
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

/// Convert a CLI command to a JSON-RPC method name string.
fn command_to_tool_name(command: &Commands) -> &'static str {
    match command {
        Commands::Read(_) => "read",
        Commands::Patch(_) => "patch",
        Commands::Write(_) => "write",
        Commands::FindBlock(_) => "find_block",
        Commands::Guide(_) | Commands::Serve(_) | Commands::Mcp(_) => {
            unreachable!()
        }
    }
}

/// Get the daemon socket path from the HASHLINE_SOCKET env var.
#[cfg(unix)]
fn get_socket_env() -> Option<PathBuf> {
    std::env::var("HASHLINE_SOCKET").ok().map(PathBuf::from)
}

/// Get the daemon URL from the HASHLINE_URL env var.
fn get_url_env() -> Option<String> {
    std::env::var("HASHLINE_URL").ok()
}

/// Route a CLI command through a Unix socket daemon.
#[cfg(unix)]
fn route_via_socket(cli: &Cli, socket_path: &Path) -> Result<i32, String> {
    let stream = UnixStream::connect(socket_path)
        .map_err(|e| format!("cannot connect to daemon at {}: {e}", socket_path.display()))?;

    let mut reader = io::BufReader::new(
        stream
            .try_clone()
            .map_err(|e| format!("clone error: {e}"))?,
    );
    let mut writer = &stream;

    let tool_name = command_to_tool_name(&cli.command);

    // Serialize the command struct as JSON arguments
    // We need to serialize the inner command struct.
    let arguments =
        serialize_command_args(&cli.command).map_err(|e| format!("serialization error: {e}"))?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        },
        "id": 1,
    });

    let request_str = serde_json::to_string(&request).map_err(|e| format!("json error: {e}"))?;

    writer
        .write_all(request_str.as_bytes())
        .map_err(|e| format!("write error: {e}"))?;
    writer
        .write_all(b"\n")
        .map_err(|e| format!("write error: {e}"))?;
    writer.flush().map_err(|e| format!("flush error: {e}"))?;

    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| format!("read error: {e}"))?;

    let response: Value =
        serde_json::from_str(&response_line).map_err(|e| format!("parse response error: {e}"))?;

    if let Some(error) = response.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        return Err(msg.to_string());
    }

    // Extract structured content from the response
    let result = response.get("result");
    if let Some(content) = result.and_then(|r| r.get("structuredContent")) {
        // Print stdout if present
        if let Some(stdout_text) = content.get("stdout").and_then(|v| v.as_str()) {
            if !stdout_text.is_empty() {
                print!("{stdout_text}");
            }
        }
        // Print stderr if present
        if let Some(stderr_text) = content.get("stderr").and_then(|v| v.as_str()) {
            if !stderr_text.is_empty() {
                eprint!("{stderr_text}");
            }
        }
        // Print data as JSON if present and no stdout
        if let Some(data) = content.get("data") {
            if content
                .get("stdout")
                .and_then(|v| v.as_str())
                .is_none_or(|s| s.is_empty())
            {
                let _ = serde_json::to_writer(std::io::stdout().lock(), data);
                println!();
            }
        }
        // Fallback: structuredContent.content[].text (MCP content format)
        if content
            .get("stdout")
            .and_then(|v| v.as_str())
            .is_none_or(|s| s.is_empty())
            && content.get("data").is_none()
        {
            if let Some(content_arr) = content.get("content").and_then(|c| c.as_array()) {
                for item in content_arr {
                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                        print!("{text}");
                    }
                }
            }
        }
        let exit_code = content
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        Ok(exit_code)
    } else if let Some(content_arr) = result
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in content_arr {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                print!("{text}");
            }
        }
        Ok(0)
    } else {
        Ok(0)
    }
}

/// Route a CLI command through an HTTP daemon.
fn route_via_http(cli: &Cli, url: &str) -> Result<i32, String> {
    // Parse URL to get host and port
    let url_parts: Vec<&str> = url.trim_start_matches("http://").split(':').collect();
    let host = url_parts.first().copied().unwrap_or("127.0.0.1");
    let port: u16 = url_parts
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(17300);

    let addr = format!("{host}:{port}");
    let mut stream = std::net::TcpStream::connect(&addr)
        .map_err(|e| format!("cannot connect to daemon at {addr}: {e}"))?;

    let tool_name = command_to_tool_name(&cli.command);
    let arguments =
        serialize_command_args(&cli.command).map_err(|e| format!("serialization error: {e}"))?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        },
        "id": 1,
    });

    let body = serde_json::to_string(&request).map_err(|e| format!("json error: {e}"))?;

    let http_request = format!(
        "POST /rpc HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body,
    );

    use std::io::Write;
    stream
        .write_all(http_request.as_bytes())
        .map_err(|e| format!("write error: {e}"))?;

    let mut reader = io::BufReader::new(&stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| format!("read error: {e}"))?;

    // Read headers
    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|e| format!("read header error: {e}"))?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(len_str) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = len_str.trim().parse().unwrap_or(0);
        }
    }

    let mut body_buf = vec![0u8; content_length];
    reader
        .read_exact(&mut body_buf)
        .map_err(|e| format!("read body error: {e}"))?;

    let response: Value =
        serde_json::from_slice(&body_buf).map_err(|e| format!("parse response error: {e}"))?;

    if let Some(error) = response.get("error") {
        let msg = error["message"].as_str().unwrap_or("unknown error");
        return Err(msg.to_string());
    }

    let result = response.get("result");

    // 1. Preferred path: daemon returned a structured `{ stdout, stderr,
    //    exit_code, data }` envelope. Honor stdout/stderr verbatim and
    //    fall back to `data` JSON when stdout is empty.
    if let Some(content) = result.and_then(|r| r.get("structuredContent")) {
        let has_stdout = !content
            .get("stdout")
            .and_then(|v| v.as_str())
            .is_none_or(|s| s.is_empty());
        if let Some(stdout_text) = content.get("stdout").and_then(|v| v.as_str()) {
            if !stdout_text.is_empty() {
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
            } else if let Some(content_arr) = content.get("content").and_then(|c| c.as_array()) {
                // 2. structuredContent.content[].text (legacy 0.7.0 shape)
                for item in content_arr {
                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                        print!("{text}");
                    }
                }
            }
        }
        let exit_code = content
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        return Ok(exit_code);
    }

    // 3. Daemon returned the raw MCP `content` array
    //    (`{"result": {"content": [{"type": "text", "text": "..."}]}}`).
    //    This is the shape produced by `serve.rs handle_http` for the
    //    `/rpc` endpoint, and the shape the legacy 0.7.5 fix relied on.
    if let Some(content_arr) = result
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in content_arr {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                print!("{text}");
            }
        }
        return Ok(0);
    }

    Ok(0)
}

/// Serialize the inner command struct into JSON for HASHLINE_SOCKET routing.
fn serialize_command_args(command: &Commands) -> Result<Value, serde_json::Error> {
    match command {
        Commands::Read(cmd) => serde_json::to_value(cmd),
        Commands::Patch(cmd) => serde_json::to_value(cmd),
        Commands::Write(cmd) => serde_json::to_value(cmd),
        Commands::FindBlock(cmd) => serde_json::to_value(cmd),
        Commands::Guide(_) | Commands::Serve(_) | Commands::Mcp(_) => {
            unreachable!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{default_log_path, run, tracing_filter};
    use hashline::cli::{Cli, Commands, ReadCmd};
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
                hashline::context::OutputMode::Pretty,
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
}
