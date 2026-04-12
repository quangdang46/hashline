use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use memchr::memchr;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::document::{read_file_meta, FileMeta, NewlineStyle, SearchDocument};
use crate::error::LinehashError;
use crate::hash::{full_hash, short_from_full};
use crate::orchestration::LineView;

fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".linehash").join("daemon.sock")
}

#[derive(Clone)]
struct CachedDocument {
    content: Arc<String>,
    line_offsets: Arc<Vec<usize>>,
    #[allow(dead_code)]
    newline: NewlineStyle,
    trailing_newline: bool,
    file_meta: FileMeta,
    #[allow(dead_code)]
    loaded_at: Instant,
    access_count: u64,
}

impl CachedDocument {
    fn new(search_doc: &SearchDocument, file_meta: FileMeta) -> Self {
        CachedDocument {
            content: Arc::new(search_doc.content.clone()),
            line_offsets: Arc::new(search_doc.line_offsets.clone()),
            newline: search_doc.newline,
            trailing_newline: search_doc.trailing_newline,
            file_meta,
            loaded_at: Instant::now(),
            access_count: 0,
        }
    }

    fn touch(&mut self) {
        self.access_count += 1;
    }

    fn is_stale(&self, current_meta: &FileMeta) -> bool {
        self.file_meta.mtime_secs != current_meta.mtime_secs
            || self.file_meta.mtime_nanos != current_meta.mtime_nanos
            || self.file_meta.size != current_meta.size
    }
}

struct DocumentRegistry {
    documents: HashMap<PathBuf, CachedDocument>,
    max_documents: usize,
    total_accesses: u64,
}

impl DocumentRegistry {
    fn new(max_documents: usize) -> Self {
        DocumentRegistry {
            documents: HashMap::new(),
            max_documents,
            total_accesses: 0,
        }
    }

    fn get(&mut self, path: &Path) -> Option<CachedDocument> {
        self.total_accesses += 1;
        if let Some(doc) = self.documents.get_mut(path) {
            doc.touch();
            Some(doc.clone())
        } else {
            None
        }
    }

    fn insert(&mut self, path: PathBuf, doc: CachedDocument) {
        if self.documents.len() >= self.max_documents {
            if let Some((oldest_path, _)) = self
                .documents
                .iter()
                .min_by_key(|(_, doc)| doc.access_count)
            {
                let oldest = oldest_path.clone();
                self.documents.remove(&oldest);
            }
        }
        self.documents.insert(path, doc);
    }

    fn invalidate_if_stale(&mut self, path: &Path, current_meta: &FileMeta) -> bool {
        if let Some(doc) = self.documents.get_mut(path) {
            if doc.is_stale(current_meta) {
                self.documents.remove(path);
                return true;
            }
        }
        false
    }

    fn len(&self) -> usize {
        self.documents.len()
    }
}

type SharedRegistry = Arc<Mutex<DocumentRegistry>>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum Request {
    Ping,
    Grep {
        path: String,
        pattern: String,
        invert: bool,
        case_insensitive: bool,
    },
    Load {
        path: String,
    },
    Unload {
        path: String,
    },
    Stats,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum Response {
    Ok { data: serde_json::Value },
    Error { message: String, kind: String },
    Pong,
}

pub fn run_daemon() -> Result<(), LinehashError> {
    let socket = socket_path();
    let socket_parent = socket.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "socket parent directory not found",
        )
    })?;

    fs::create_dir_all(socket_parent).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to create socket dir: {e}"),
        )
    })?;

    let _ = fs::remove_file(&socket);

    let listener = UnixListener::bind(&socket).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to bind socket: {e}"),
        )
    })?;

    info!(path = %socket.display(), "daemon listening");

    let registry: SharedRegistry = Arc::new(Mutex::new(DocumentRegistry::new(100)));
    let registry_clone = Arc::clone(&registry);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let registry = Arc::clone(&registry_clone);
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, &registry) {
                        error!(error = %e, "connection handler error");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "incoming connection error");
            }
        }
    }

    Ok(())
}

fn handle_connection(
    stream: UnixStream,
    registry: &SharedRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut line = String::new();
    let mut stream = stream;
    loop {
        line.clear();
        let bytes_read = BufReader::new(&mut stream).read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }

        let request: Request = match serde_json::from_str(line.trim()) {
            Ok(req) => req,
            Err(e) => {
                let response = Response::Error {
                    message: format!("invalid request: {e}"),
                    kind: "parse_error".to_string(),
                };
                let response_str = serde_json::to_string(&response)?;
                stream.write_all(response_str.as_bytes())?;
                stream.write_all(b"\n")?;
                continue;
            }
        };

        let response = process_request(request, registry);
        let response_str = serde_json::to_string(&response)?;
        stream.write_all(response_str.as_bytes())?;
        stream.write_all(b"\n")?;
    }

    Ok(())
}

fn process_request(request: Request, registry: &SharedRegistry) -> Response {
    match request {
        Request::Ping => Response::Pong,

        Request::Grep {
            path,
            pattern,
            invert,
            case_insensitive,
        } => {
            let path = PathBuf::from(&path);
            let result = grep_cached(&path, &pattern, invert, case_insensitive, registry);
            match result {
                Ok(lines) => Response::Ok {
                    data: serde_json::to_value(lines).unwrap_or(serde_json::Value::Null),
                },
                Err(e) => Response::Error {
                    message: e.to_string(),
                    kind: "grep_error".to_string(),
                },
            }
        }

        Request::Load { path } => {
            let path = PathBuf::from(&path);
            match load_document(&path, registry) {
                Ok(_) => Response::Ok {
                    data: serde_json::json!({"loaded": true, "path": path.display().to_string()}),
                },
                Err(e) => Response::Error {
                    message: e.to_string(),
                    kind: "load_error".to_string(),
                },
            }
        }

        Request::Unload { path } => {
            let path = PathBuf::from(&path);
            let mut reg = registry.lock().unwrap();
            let removed = reg.documents.remove(&path).is_some();
            Response::Ok {
                data: serde_json::json!({"unloaded": removed, "path": path.display().to_string()}),
            }
        }

        Request::Stats => {
            let reg = registry.lock().unwrap();
            Response::Ok {
                data: serde_json::json!({
                    "document_count": reg.len(),
                    "total_accesses": reg.total_accesses,
                    "max_documents": reg.max_documents,
                }),
            }
        }
    }
}

fn load_document(path: &Path, registry: &SharedRegistry) -> Result<(), LinehashError> {
    let file_meta = read_file_meta(path)?;
    let search_doc = SearchDocument::load(path)?;

    {
        let mut reg = registry.lock().unwrap();
        reg.invalidate_if_stale(path, &file_meta);
    }

    let cached = CachedDocument::new(&search_doc, file_meta);
    let mut reg = registry.lock().unwrap();
    reg.insert(path.to_path_buf(), cached);

    debug!(path = %path.display(), "document loaded into cache");
    Ok(())
}

fn grep_cached(
    path: &Path,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
    registry: &SharedRegistry,
) -> Result<Vec<LineView>, LinehashError> {
    let file_meta = read_file_meta(path)?;

    let cached = {
        let mut reg = registry.lock().unwrap();
        reg.invalidate_if_stale(path, &file_meta);
        if let Some(c) = reg.get(path) {
            c
        } else {
            drop(reg);
            let search_doc = SearchDocument::load(path)?;
            let mut cached = CachedDocument::new(&search_doc, file_meta);
            cached.touch();
            let mut reg = registry.lock().unwrap();
            reg.insert(path.to_path_buf(), cached.clone());
            cached
        }
    };

    grep_lines_cached(&cached, pattern, invert, case_insensitive)
}

fn grep_lines_cached(
    doc: &CachedDocument,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
) -> Result<Vec<LineView>, LinehashError> {
    let pattern_bytes = pattern.as_bytes();
    let pat_len = pattern_bytes.len();
    let mut results = Vec::new();

    if !case_insensitive && pat_len == 1 && !contains_regex_metacharacters(pattern) {
        let byte = pattern_bytes[0];
        for (line_idx, &start) in doc.line_offsets.iter().enumerate() {
            let end = if line_idx + 1 < doc.line_offsets.len() {
                doc.line_offsets[line_idx + 1]
            } else {
                doc.content.len()
            };
            let line_end = if doc.trailing_newline
                && end > start
                && doc.content.as_bytes()[end.saturating_sub(1)] == b'\n'
            {
                end - 1
            } else {
                end.min(doc.content.len())
            };
            let line_content = &doc.content[start..line_end];

            let is_match = memchr(byte, line_content.as_bytes()).is_some();
            let include = if invert { !is_match } else { is_match };
            if include {
                let full_hash = full_hash(line_content);
                let short_hash = short_from_full(full_hash);
                results.push(LineView {
                    n: line_idx + 1,
                    hash: format_hash(short_hash),
                    content: line_content.to_string(),
                });
            }
        }
    } else {
        for (line_idx, &start) in doc.line_offsets.iter().enumerate() {
            let end = if line_idx + 1 < doc.line_offsets.len() {
                doc.line_offsets[line_idx + 1]
            } else {
                doc.content.len()
            };
            let line_end = if doc.trailing_newline
                && end > start
                && doc.content.as_bytes()[end.saturating_sub(1)] == b'\n'
            {
                end - 1
            } else {
                end.min(doc.content.len())
            };
            let line_content = &doc.content[start..line_end];

            let is_match = if case_insensitive {
                line_content
                    .to_lowercase()
                    .contains(&pattern.to_lowercase())
            } else if pat_len == 1 {
                memchr(pattern_bytes[0], line_content.as_bytes()).is_some()
            } else if pat_len <= line_content.len() {
                line_content
                    .as_bytes()
                    .windows(pat_len)
                    .any(|w| w == pattern_bytes)
            } else {
                false
            };

            let include = if invert { !is_match } else { is_match };
            if include {
                let full_hash = full_hash(line_content);
                let short_hash = short_from_full(full_hash);
                results.push(LineView {
                    n: line_idx + 1,
                    hash: format_hash(short_hash),
                    content: line_content.to_string(),
                });
            }
        }
    }

    Ok(results)
}

fn contains_regex_metacharacters(s: &str) -> bool {
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
            | '"' => return true,
            _ => {}
        }
    }
    false
}

fn format_hash(short_hash: u8) -> String {
    format!("{:02x}", short_hash)
}

pub fn client_request(request: &Request) -> Result<serde_json::Value, LinehashError> {
    let socket_path = socket_path();

    let mut stream = UnixStream::connect(&socket_path).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            format!("daemon not running at {}: {e}", socket_path.display()),
        )
    })?;

    let request_json = serde_json::to_string(request).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("serialization error: {e}"),
        )
    })?;

    stream
        .write_all(request_json.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("write error: {e}")))?;
    stream
        .write_all(b"\n")
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("write error: {e}")))?;

    let mut response_line = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut response_line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("read error: {e}")))?;

    let response: Response = serde_json::from_str(response_line.trim()).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("invalid response: {e}"))
    })?;

    match response {
        Response::Ok { data } => Ok(data),
        Response::Error { message, kind } => Err(LinehashError::ServerError { message, kind }),
        Response::Pong => Ok(serde_json::json!({"pong": true})),
    }
}

pub fn is_daemon_running() -> bool {
    let socket_path = socket_path();
    UnixStream::connect(&socket_path).is_ok()
}

pub fn start_daemon() -> Result<std::process::Child, LinehashError> {
    use std::process::Command;

    if is_daemon_running() {
        return Err(LinehashError::ServerError {
            message: "daemon already running".to_string(),
            kind: "already_running".to_string(),
        });
    }

    let child = Command::new(std::env::current_exe().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("failed to get exe: {e}"))
    })?)
    .arg("daemon")
    .spawn()
    .map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to spawn daemon: {e}"),
        )
    })?;

    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if is_daemon_running() {
            info!("daemon started successfully");
            return Ok(child);
        }
    }

    Err(LinehashError::ServerError {
        message: "daemon failed to start".to_string(),
        kind: "startup_failed".to_string(),
    })
}

pub fn ensure_daemon_running() -> Result<(), LinehashError> {
    if is_daemon_running() {
        return Ok(());
    }

    let exe = std::env::current_exe().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("failed to get exe: {e}"))
    })?;

    let daemon_exe = exe.display().to_string();

    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("sh")
            .args([
                "-c",
                &format!("nohup {} daemon </dev/null >/dev/null 2>&1 &", daemon_exe),
            ])
            .spawn()
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to spawn daemon: {e}"),
                )
            })?;
    }

    #[cfg(not(unix))]
    {
        start_daemon()?;
    }

    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if is_daemon_running() {
            return Ok(());
        }
    }

    Err(LinehashError::ServerError {
        message: "daemon failed to start".to_string(),
        kind: "startup_failed".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_regex_metacharacters() {
        assert!(contains_regex_metacharacters("foo.bar"));
        assert!(contains_regex_metacharacters("foo+bar"));
        assert!(!contains_regex_metacharacters("foobar"));
        assert!(!contains_regex_metacharacters("foo bar"));
    }

    #[test]
    fn test_format_hash() {
        assert_eq!(format_hash(0xff), "ff");
        assert_eq!(format_hash(0x0a), "0a");
        assert_eq!(format_hash(0x00), "00");
    }
}
