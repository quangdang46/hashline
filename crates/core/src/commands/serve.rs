use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

// Mutable statics used by the SIGINT/SIGTERM/SIGHUP cleanup handler.
// They are only written once during daemon startup and read by async
// signal handlers, so plain globals are sufficient.
#[cfg(unix)]
static mut CLEANUP_SOCKET: *const libc::c_char = std::ptr::null();
#[cfg(unix)]
static mut CLEANUP_PID: *const libc::c_char = std::ptr::null();

use crate::cli::ServeCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;
use crate::mcp;

/// Default Unix socket path under ~/.hashline/
pub fn default_daemon_socket() -> PathBuf {
    let home = crate::config::hashline_home_dir().unwrap_or_else(std::env::temp_dir);
    home.join(".hashline").join("daemon.sock")
}

// ---------------------------------------------------------------------------
// HTTP bearer-token authentication (bd-serve-http-unauthenticated-06tw)
// ---------------------------------------------------------------------------
//
// `serve --http` exposes the full JSON-RPC file-mutation surface on
// 127.0.0.1. Without authentication any web page could POST to it via a
// no-cors form/fetch (CSRF) and patch/write/remove arbitrary files as the
// user. The daemon therefore requires `Authorization: Bearer <token>` on
// every request, where the token comes from (in order):
//
//   1. `serve --token <TOKEN>`
//   2. `HASHLINE_TOKEN` env var
//   3. `~/.hashline/http.token` — generated once (0600) on first HTTP serve
//
// The persisted file is the discovery path for agents: `hashline` CLI
// commands routed via `HASHLINE_URL` read the same file (or env) and send
// the header automatically. Requiring `Content-Type: application/json`
// forces browsers into a CORS preflight they cannot satisfy, and any
// non-empty `Origin` header is rejected outright.

/// Persisted bearer-token path under the hashline home dir.
pub fn http_token_path() -> PathBuf {
    let home = crate::config::hashline_home_dir().unwrap_or_else(std::env::temp_dir);
    home.join(".hashline").join("http.token")
}

/// Read the bearer token a *client* should send: `HASHLINE_TOKEN` env,
/// then the persisted `~/.hashline/http.token`. Never creates the file —
/// returns `None` when no token is configured.
pub fn read_http_token() -> Option<String> {
    if let Ok(v) = std::env::var("HASHLINE_TOKEN") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    std::fs::read_to_string(http_token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the token `serve --http` requires. `explicit` is `--token`;
/// falls back to `HASHLINE_TOKEN`, then the persisted file — generating
/// and storing a random token (mode 0600) on first use. The effective
/// token is always written back to the file so agents have a single
/// discovery path regardless of how it was supplied.
pub fn resolve_http_token(explicit: Option<&str>) -> Result<(String, PathBuf), HashlineError> {
    let path = http_token_path();
    let token = match explicit {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => match std::env::var("HASHLINE_TOKEN") {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => match std::fs::read_to_string(&path) {
                Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
                _ => generate_http_token(),
            },
        },
    };
    write_token_file(&path, &token)?;
    Ok((token, path))
}

/// 48 chars of alphanumeric randomness — ~286 bits, far beyond guessing
/// range even for an attacker who can reach the port.
fn generate_http_token() -> String {
    (0..48).map(|_| fastrand::alphanumeric()).collect()
}

/// Write `token` to `path` with owner-only permissions (0600 on unix).
fn write_token_file(path: &std::path::Path, token: &str) -> Result<(), HashlineError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            HashlineError::Io(io::Error::other(format!(
                "failed to create token directory: {e}"
            )))
        })?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(token.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(())
}

/// Constant-time bearer-token comparison. Length is compared first
/// (it is not secret — the token file is 0600 and the env var is
/// user-controlled), then bytes are folded so early-exit timing cannot
/// leak prefix matches.
fn token_eq(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// HTTP hardening limits (bd-serve-http-unauthenticated-06tw).
/// Total request-head bytes (request line + headers). Anything larger is
/// rejected with 431 before a single body byte is read.
const HTTP_MAX_HEADER_BYTES: usize = 64 * 1024;
/// Maximum JSON-RPC body. Larger requests get 413 — the old code trusted
/// Content-Length blindly (and `unwrap_or(0)` on a malformed value).
const HTTP_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Per-read/write timeout so a slowloris-style client cannot park a
/// handler thread forever.
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Why the request head was rejected before dispatch.
#[derive(Debug, PartialEq, Eq)]
enum HttpReject {
    /// Head exceeded [`HTTP_MAX_HEADER_BYTES`] or a single header line
    /// exceeded the per-line cap.
    HeadersTooLarge,
    /// Head was truncated by EOF before the blank-line terminator.
    HeadersIncomplete,
    /// Non-POST method.
    MethodNotAllowed,
    /// Missing/wrong `Authorization: Bearer`.
    Unauthorized,
    /// Non-empty `Origin` header (browser cross-site request).
    ForbiddenOrigin,
    /// Missing or non-JSON `Content-Type`.
    UnsupportedMediaType,
    /// Malformed/duplicate/missing `Content-Length`, or a
    /// `Transfer-Encoding` we do not support.
    BadContentLength,
    /// `Content-Length` over [`HTTP_MAX_BODY_BYTES`].
    PayloadTooLarge,
}

impl HttpReject {
    fn status(&self) -> &'static str {
        match self {
            HttpReject::HeadersTooLarge => "431 Request Header Fields Too Large",
            HttpReject::HeadersIncomplete => "400 Bad Request",
            HttpReject::MethodNotAllowed => "405 Method Not Allowed",
            HttpReject::Unauthorized => "401 Unauthorized",
            HttpReject::ForbiddenOrigin => "403 Forbidden",
            HttpReject::UnsupportedMediaType => "415 Unsupported Media Type",
            HttpReject::BadContentLength => "400 Bad Request",
            HttpReject::PayloadTooLarge => "413 Payload Too Large",
        }
    }
}

/// Write a minimal HTTP error response and end the connection.
fn http_error(stream: &mut TcpStream, reject: &HttpReject) -> io::Result<()> {
    let body = format!("{{\"error\":\"{}\"}}", reject.status());
    let mut response = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        reject.status(),
        body.len(),
    );
    if matches!(reject, HttpReject::Unauthorized) {
        response.push_str("WWW-Authenticate: Bearer realm=\"hashline\"\r\n");
    }
    response.push_str("\r\n");
    response.push_str(&body);
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Read the request head (request line + headers up to the blank line)
/// byte-at-a-time through the buffered reader, bounded by
/// [`HTTP_MAX_HEADER_BYTES`]. Returns the raw head bytes.
fn read_http_head(reader: &mut io::BufReader<&TcpStream>) -> Result<Vec<u8>, HttpReject> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if head.len() >= HTTP_MAX_HEADER_BYTES {
            return Err(HttpReject::HeadersTooLarge);
        }
        match reader.read(&mut byte) {
            Ok(0) => return Err(HttpReject::HeadersIncomplete),
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
                    return Ok(head);
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                return Err(HttpReject::HeadersIncomplete);
            }
            Err(_) => return Err(HttpReject::HeadersIncomplete),
        }
    }
}

/// Parse and enforce the HTTP request policy. Returns the validated
/// `Content-Length` on success.
fn check_http_request(head: &[u8], token: &str) -> Result<usize, HttpReject> {
    let text = String::from_utf8_lossy(head);
    // `str::lines` splits on '\n' and strips a trailing '\r', so both
    // CRLF and bare-LF heads parse identically.
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    if method != "POST" {
        return Err(HttpReject::MethodNotAllowed);
    }

    let mut authorization: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut content_length: Option<usize> = None;
    let mut transfer_encoding = false;

    for line in lines {
        if line.is_empty() {
            break; // end of headers
        }
        let Some((name, value)) = line.split_once(':') else {
            continue; // tolerate malformed header lines; body parse still gates
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "authorization" => authorization = Some(value.to_string()),
            "content-type" => content_type = Some(value.to_string()),
            "origin" => origin = Some(value.to_string()),
            "content-length" => {
                // Strict parse: digits only, no sign, no whitespace junk,
                // no duplicates (a second CL would overwrite silently).
                if content_length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(HttpReject::BadContentLength);
                }
                let len = value
                    .parse::<usize>()
                    .map_err(|_| HttpReject::BadContentLength)?;
                content_length = Some(len);
            }
            "transfer-encoding" => transfer_encoding = true,
            _ => {}
        }
    }

    // 1. Authentication first — an unauthenticated request learns nothing
    //    about which other checks would have failed.
    let expected = format!("Bearer {token}");
    match authorization.as_deref() {
        Some(provided) if token_eq(provided, &expected) => {}
        _ => return Err(HttpReject::Unauthorized),
    }

    // 2. Browser CSRF hardening: browsers always send Origin on
    //    cross-site POSTs; legitimate local agents do not.
    if origin.as_deref().is_some_and(|o| !o.is_empty()) {
        return Err(HttpReject::ForbiddenOrigin);
    }

    // 3. Content-Type: application/json forces a CORS preflight for
    //    browser form/fetch posts (which cannot set it without preflight).
    // `get(..16)` returns None on a non-char-boundary instead of
    // panicking on hostile non-ASCII input.
    let json_ct = content_type.as_deref().is_some_and(|ct| {
        ct.get(..16)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("application/json"))
    });
    if !json_ct {
        return Err(HttpReject::UnsupportedMediaType);
    }

    // 4. Body framing: exactly one strict Content-Length, no chunked.
    if transfer_encoding {
        return Err(HttpReject::BadContentLength);
    }
    let len = content_length.ok_or(HttpReject::BadContentLength)?;
    if len > HTTP_MAX_BODY_BYTES {
        return Err(HttpReject::PayloadTooLarge);
    }
    Ok(len)
}

/// Run the daemon: listen on a Unix socket or HTTP port,
/// accepting connections and serving JSON-RPC requests.
pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ServeCmd,
) -> Result<(), HashlineError> {
    let socket_path = cmd
        .socket
        .clone()
        .or_else(|| Some(default_daemon_socket()))
        .expect("default socket path");
    let http_port = cmd.http;
    let detach = cmd.detach;
    let pid_file = cmd.pid_file.clone().or_else(|| {
        let home = crate::config::hashline_home_dir().unwrap_or_else(std::env::temp_dir);
        Some(home.join(".hashline").join("daemon.pid"))
    });

    // Ensure ~/.hashline/ exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            HashlineError::Io(io::Error::other(format!(
                "failed to create daemon directory: {e}"
            )))
        })?;
    }
    if let Some(ref pid_file_path) = pid_file {
        if let Some(parent) = pid_file_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    // Lock the socket file to prevent multiple daemon instances
    let lock_file_path = socket_path.with_extension("sock.lock");
    #[cfg_attr(not(unix), allow(unused_variables))]
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&lock_file_path)
        .map_err(|e| {
            HashlineError::Io(io::Error::other(format!("failed to open lock file: {e}")))
        })?;

    // Try to acquire an exclusive lock (non-blocking)
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = lock_file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Err(HashlineError::Io(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "daemon is already running (lock held at {})",
                        lock_file_path.display()
                    ),
                )));
            }
            return Err(HashlineError::Io(err));
        }
    }

    // Clean up old socket file if it exists
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }

    // --detach: fork to background (Unix only)
    if detach {
        #[cfg(unix)]
        {
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return Err(HashlineError::Io(io::Error::other("fork failed")));
            }
            if pid > 0 {
                // Parent process exits
                if let Some(ref pid_file_path) = pid_file {
                    if let Err(e) = std::fs::write(pid_file_path, pid.to_string()) {
                        eprintln!("warning: failed to write PID file: {e}");
                    }
                }
                std::process::exit(0);
            }
            // Child continues
            // Create a new session (detach from terminal)
            unsafe {
                libc::setsid();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = detach;
            // On non-Unix, --detach is a no-op (would need Windows service API)
        }
    }

    // Remove old socket file again after potential fork
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }

    // Install signal handlers so SIGINT/SIGTERM/SIGHUP clean up the
    // lock + pid + socket files. Without this, an aborted daemon
    // leaves stale artifacts in `~/.hashline/` that block the next
    // start ("daemon is already running" / "address in use" errors).
    #[cfg(unix)]
    {
        #[allow(clippy::missing_transmute_annotations)]
        extern "C" fn cleanup_signal(_sig: libc::c_int) {
            // We are in a signal handler: only async-signal-safe operations.
            // `_exit` is safe and prevents destructors; the lock file is
            // released by the kernel when the process exits, so we only
            // need to remove the explicit pid and socket files. We use
            // unlink via a fixed path that the helper sets below.
            unsafe {
                libc::unlink(CLEANUP_SOCKET);
                if !CLEANUP_PID.is_null() {
                    libc::unlink(CLEANUP_PID);
                }
                libc::_exit(0);
            }
        }
        // Stash the paths the handler will use. We use leaked CStrings
        // because the signal handler may fire at any time and cannot
        // borrow from the stack.
        let sock_c =
            std::ffi::CString::new(socket_path.to_string_lossy().as_bytes()).unwrap_or_default();
        let pid_c = pid_file
            .as_ref()
            .and_then(|p| std::ffi::CString::new(p.to_string_lossy().as_bytes()).ok());
        unsafe {
            CLEANUP_SOCKET = sock_c.into_raw();
            CLEANUP_PID = pid_c
                .map(|c| c.into_raw() as *const _)
                .unwrap_or(std::ptr::null());
            {
                libc::signal(
                    libc::SIGINT,
                    cleanup_signal as *const () as libc::sighandler_t,
                );
                libc::signal(
                    libc::SIGTERM,
                    cleanup_signal as *const () as libc::sighandler_t,
                );
                libc::signal(
                    libc::SIGHUP,
                    cleanup_signal as *const () as libc::sighandler_t,
                );
            }
        }
    }

    // Thread pool for handling connections (simple: spawn a thread per connection)
    // We use std::thread so there's no async runtime dependency.
    use std::thread;

    // HTTP mode requires a bearer token: resolve it up front so a token
    // file problem fails before the listener binds, and tell the operator
    // where agents can read it.
    let http_token: Option<Arc<str>> = if http_port.is_some() {
        let (token, token_path) = resolve_http_token(cmd.token.as_deref())?;
        writeln!(
            ctx.stderr(),
            "hashline daemon HTTP auth: bearer token at {} \
             (send `Authorization: Bearer <token>`; also HASHLINE_TOKEN / --token)",
            token_path.display()
        )?;
        Some(Arc::from(token))
    } else {
        None
    };

    if let Some(port) = http_port {
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr).map_err(|e| {
            HashlineError::Io(io::Error::other(format!(
                "failed to bind HTTP on {addr}: {e}"
            )))
        })?;

        writeln!(ctx.stderr(), "hashline daemon listening on http://{addr}")?;

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let token = http_token
                        .clone()
                        .expect("http_token resolved when http_port is set");
                    thread::spawn(move || {
                        if let Err(e) = handle_http(stream, &token) {
                            eprintln!("http handler error: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("accept error: {e}");
                }
            }
        }
    } else {
        // Unix socket mode
        #[cfg(unix)]
        {
            let listener = UnixListener::bind(&socket_path).map_err(|e| {
                HashlineError::Io(io::Error::other(format!(
                    "failed to bind socket at {}: {e}",
                    socket_path.display()
                )))
            })?;

            // Set permissions so only the owner can connect
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).ok();

            writeln!(
                ctx.stderr(),
                "hashline daemon listening on {}",
                socket_path.display()
            )?;

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        thread::spawn(|| {
                            if let Err(e) = handle_unix(stream) {
                                eprintln!("unix handler error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("accept error: {e}");
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = socket_path;
            return Err(HashlineError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unix socket mode is not supported on this platform; use --http instead",
            )));
        }
    }

    Ok(())
}

/// Handle a single Unix socket connection: read JSON-RPC lines,
/// process each, write response back.
#[cfg(unix)]
fn handle_unix(mut stream: std::os::unix::net::UnixStream) -> io::Result<()> {
    // Use a separate reference for reading to avoid borrowing conflicts
    let mut session = mcp::Session::new();
    let mut line = String::new();
    let mut reader = io::BufReader::new(stream.try_clone()?);

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break; // connection closed
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: mcp::JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(error) => {
                mcp::write_error(
                    &mut stream,
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

        let response = mcp::handle_request(&request, &mut session);
        serde_json::to_writer(&mut stream, &response)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }

    Ok(())
}

/// Handle a single HTTP connection: enforce the bearer-token/CSRF/size
/// policy, parse the JSON-RPC body, process, return the response.
fn handle_http(mut stream: TcpStream, token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(HTTP_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_IO_TIMEOUT))?;
    let mut reader = io::BufReader::new(&stream);

    let head = match read_http_head(&mut reader) {
        Ok(head) => head,
        Err(reject) => return http_error(&mut stream, &reject),
    };
    let content_length = match check_http_request(&head, token) {
        Ok(len) => len,
        Err(reject) => return http_error(&mut stream, &reject),
    };

    // Read the body — bounded by the validated Content-Length. A client
    // that goes away mid-body surfaces as an io error; there is nothing
    // left to answer.
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let body_str = String::from_utf8(body).unwrap_or_default();

    let mut session = mcp::Session::new();

    let request: mcp::JsonRpcRequest = match serde_json::from_str(&body_str) {
        Ok(req) => req,
        Err(error) => {
            let response = mcp::JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(mcp::JsonRpcError {
                    code: -32700,
                    message: format!("parse error: {error}"),
                    data: None,
                }),
            };
            let body_out = serde_json::to_string(&response).unwrap_or_default();
            let http_resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body_out.len(),
                body_out,
            );
            let mut stream = &stream;
            stream.write_all(http_resp.as_bytes())?;
            stream.flush()?;
            return Ok(());
        }
    };

    let response = if request.method == "tools/call"
        || request.method == "tools/list"
        || request.method == "initialize"
        || request.method == "ping"
    {
        mcp::handle_request(&request, &mut session)
    } else {
        // Handle CLI commands through the socket
        let command_name = request.method.as_str();
        let result = mcp::dispatch_tool(command_name, &request.params, &mut session);
        match result {
            Ok(payload) => {
                let text = serde_json::to_string(&payload).unwrap_or_default();
                mcp::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: Some(json!({
                        "content": [{"type": "text", "text": text}],
                        "structuredContent": payload,
                    })),
                    error: None,
                }
            }
            Err(e) => mcp::JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(e),
            },
        }
    };

    let body_out = serde_json::to_string(&response).unwrap_or_default();
    let http_resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body_out.len(),
        body_out,
    );

    let mut stream = &stream;
    stream.write_all(http_resp.as_bytes())?;
    stream.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "test-token-123";

    /// Build a minimal valid POST head; override individual headers via
    /// `extra` lines (each a full `Name: value` line, `\n`-terminated).
    fn head(extra: &str) -> String {
        format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n{extra}\r\n"
        )
    }

    #[test]
    fn valid_request_passes_and_returns_content_length() {
        let h = head("");
        assert_eq!(check_http_request(h.as_bytes(), TOKEN), Ok(2));
    }

    #[test]
    fn missing_or_wrong_authorization_is_unauthorized() {
        let no_auth =
            b"POST /rpc HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n"
                .to_vec();
        assert!(matches!(
            check_http_request(&no_auth, TOKEN),
            Err(HttpReject::Unauthorized)
        ));

        let wrong = head("").replace("Bearer test-token-123", "Bearer wrong");
        assert!(matches!(
            check_http_request(wrong.as_bytes(), TOKEN),
            Err(HttpReject::Unauthorized)
        ));

        // A bare token without the Bearer scheme must not authenticate.
        let bare = head("").replace("Bearer test-token-123", "test-token-123");
        assert!(matches!(
            check_http_request(bare.as_bytes(), TOKEN),
            Err(HttpReject::Unauthorized)
        ));
    }

    #[test]
    fn non_post_method_is_rejected() {
        let h = head("").replacen("POST /rpc", "GET /rpc", 1);
        assert!(matches!(
            check_http_request(h.as_bytes(), TOKEN),
            Err(HttpReject::MethodNotAllowed)
        ));
    }

    #[test]
    fn non_empty_origin_is_forbidden() {
        // Even a syntactically valid request from a browser context is
        // rejected — Origin is only sent on cross-site posts.
        let h = head("Origin: https://evil.example\r\n");
        assert!(matches!(
            check_http_request(h.as_bytes(), TOKEN),
            Err(HttpReject::ForbiddenOrigin)
        ));
        // An empty Origin value is tolerated.
        let h = head("Origin:\r\n");
        assert_eq!(check_http_request(h.as_bytes(), TOKEN), Ok(2));
    }

    #[test]
    fn content_type_must_be_json() {
        let form = head("").replacen(
            "Content-Type: application/json",
            "Content-Type: text/plain",
            1,
        );
        assert!(matches!(
            check_http_request(form.as_bytes(), TOKEN),
            Err(HttpReject::UnsupportedMediaType)
        ));
        let missing = b"POST /rpc HTTP/1.1\r\nAuthorization: Bearer test-token-123\r\nContent-Length: 2\r\n\r\n".to_vec();
        assert!(matches!(
            check_http_request(&missing, TOKEN),
            Err(HttpReject::UnsupportedMediaType)
        ));
        // Parameters after the media type are fine.
        let with_param = head("").replacen(
            "Content-Type: application/json",
            "Content-Type: application/json; charset=utf-8",
            1,
        );
        assert_eq!(check_http_request(with_param.as_bytes(), TOKEN), Ok(2));
    }

    #[test]
    fn content_length_is_strict() {
        // Missing entirely.
        let missing = b"POST /rpc HTTP/1.1\r\nAuthorization: Bearer test-token-123\r\nContent-Type: application/json\r\n\r\n".to_vec();
        assert!(matches!(
            check_http_request(&missing, TOKEN),
            Err(HttpReject::BadContentLength)
        ));
        // Non-numeric (the old code's unwrap_or(0) silently accepted this).
        let bad = head("").replacen("Content-Length: 2", "Content-Length: abc", 1);
        assert!(matches!(
            check_http_request(bad.as_bytes(), TOKEN),
            Err(HttpReject::BadContentLength)
        ));
        // Duplicate header.
        let dup = head("Content-Length: 2\r\n");
        assert!(matches!(
            check_http_request(dup.as_bytes(), TOKEN),
            Err(HttpReject::BadContentLength)
        ));
        // Chunked transfer encoding is not supported.
        let chunked = head("Transfer-Encoding: chunked\r\n");
        assert!(matches!(
            check_http_request(chunked.as_bytes(), TOKEN),
            Err(HttpReject::BadContentLength)
        ));
        // Over the 16 MiB cap.
        let huge = head("").replacen("Content-Length: 2", "Content-Length: 16777217", 1);
        assert!(matches!(
            check_http_request(huge.as_bytes(), TOKEN),
            Err(HttpReject::PayloadTooLarge)
        ));
        // Exactly at the cap is allowed.
        let at_cap = head("").replacen("Content-Length: 2", "Content-Length: 16777216", 1);
        assert_eq!(
            check_http_request(at_cap.as_bytes(), TOKEN),
            Ok(16 * 1024 * 1024)
        );
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let h = b"POST /rpc HTTP/1.1\r\nauthorization: Bearer test-token-123\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n".to_vec();
        assert_eq!(check_http_request(&h, TOKEN), Ok(2));
    }

    #[test]
    fn token_eq_is_exact() {
        assert!(token_eq("Bearer tok", "Bearer tok"));
        assert!(!token_eq("Bearer tok", "Bearer tokx"));
        assert!(!token_eq("Bearer tok", "bearer tok"));
        assert!(!token_eq("", "Bearer tok"));
    }

    #[test]
    fn read_http_head_enforces_size_cap() {
        // A head larger than HTTP_MAX_HEADER_BYTES must be rejected with
        // 431 before the blank-line terminator arrives.
        let mut big = b"POST /rpc HTTP/1.1\r\nX-Pad: ".to_vec();
        big.extend(std::iter::repeat_n(b'a', HTTP_MAX_HEADER_BYTES + 1));
        // Feed through a real TcpStream pair so BufReader semantics apply.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(&big).unwrap();
            s.flush().unwrap();
            // Keep the socket open briefly so the server sees the bytes.
            std::thread::sleep(Duration::from_millis(200));
        });
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = io::BufReader::new(&stream);
        let err = read_http_head(&mut reader).unwrap_err();
        assert!(matches!(err, HttpReject::HeadersTooLarge));
        client.join().unwrap();
    }
}
