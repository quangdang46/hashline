use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

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
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".hashline").join("daemon.sock")
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
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/tmp".into());
        Some(PathBuf::from(home).join(".hashline").join("daemon.pid"))
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
                    thread::spawn(|| {
                        if let Err(e) = handle_http(stream) {
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

/// Handle a single HTTP connection: parse minimal HTTP POST,
/// extract JSON-RPC body, process, return response.
fn handle_http(stream: TcpStream) -> io::Result<()> {
    let mut reader = io::BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse request line: GET / POST /etc
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "POST" {
        // Return 405 for non-POST
        let response = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        let mut stream = &stream;
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        return Ok(());
    }

    // Read headers to find Content-Length
    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(len_str) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = len_str.trim().parse().unwrap_or(0);
        }
    }

    // Read body
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
