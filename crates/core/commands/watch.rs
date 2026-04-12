use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;

use crate::cli::WatchCmd;
use crate::context::CommandContext;
use crate::document::{Document, format_short_hash};
use crate::error::LinehashError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: WatchCmd,
) -> Result<(), LinehashError> {
    let once = cmd.once || !cmd.continuous;
    watch_file(&cmd.file, once, cmd.json, ctx.stdout())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Changed,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HashDiff {
    pub line_no: usize,
    pub kind: DiffKind,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub content: String,
}

#[derive(Debug, Serialize)]
struct WatchEvent<'a> {
    timestamp: u64,
    event: &'static str,
    path: &'a str,
    changes: &'a [HashDiff],
    total_lines: usize,
}

pub fn watch_file(
    path: &Path,
    once: bool,
    json: bool,
    writer: &mut impl Write,
) -> Result<(), LinehashError> {
    let mut old_doc = Document::load(path)?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = new_watcher(tx)?;
    let watch_root = path.parent().unwrap_or_else(|| Path::new("."));
    watcher
        .watch(watch_root, RecursiveMode::NonRecursive)
        .map_err(notify_error)?;

    if !json {
        writeln!(writer, "Watching {} — Ctrl-C to stop", path.display())?;
    }

    loop {
        let event = rx
            .recv()
            .map_err(|error| LinehashError::Io(std::io::Error::other(error)))?;
        let event = event.map_err(notify_error)?;
        if !is_relevant_event(&event.kind) {
            continue;
        }
        if !event_targets_path(&event.paths, path) {
            continue;
        }

        let new_doc = Document::load(path)?;
        let changes = diff_documents(&old_doc, &new_doc);
        emit_event(writer, path, &changes, new_doc.len(), json)?;
        old_doc = new_doc;

        if once {
            break;
        }
    }

    Ok(())
}

fn new_watcher(
    tx: mpsc::Sender<Result<notify::Event, notify::Error>>,
) -> Result<RecommendedWatcher, LinehashError> {
    notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })
    .map_err(notify_error)
}

fn notify_error(error: notify::Error) -> LinehashError {
    LinehashError::Io(std::io::Error::other(error.to_string()))
}

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Create(CreateKind::Any)
            | EventKind::Create(CreateKind::File)
            | EventKind::Remove(RemoveKind::Any)
            | EventKind::Remove(RemoveKind::File)
            | EventKind::Any
    )
}

fn event_targets_path(paths: &[std::path::PathBuf], target: &Path) -> bool {
    if paths.is_empty() {
        return true;
    }

    paths.iter().any(|path| path == target)
}

fn emit_event(
    writer: &mut impl Write,
    path: &Path,
    changes: &[HashDiff],
    total_lines: usize,
    json: bool,
) -> Result<(), LinehashError> {
    if json {
        let rendered_path = path.display().to_string();
        let payload = WatchEvent {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            event: "changed",
            path: &rendered_path,
            changes,
            total_lines,
        };
        serde_json::to_writer(&mut *writer, &payload)?;
        writeln!(writer)?;
    } else if changes.is_empty() {
        writeln!(
            writer,
            "No hash changes in {} ({} lines).",
            path.display(),
            total_lines
        )?;
    } else {
        for change in changes {
            writeln!(
                writer,
                "line {} {:?}: {} -> {} | {}",
                change.line_no,
                change.kind,
                change.old_hash.as_deref().unwrap_or("∅"),
                change.new_hash.as_deref().unwrap_or("∅"),
                change.content
            )?;
        }
        writeln!(
            writer,
            "New index: {} lines, {} hash changes",
            total_lines,
            changes.len()
        )?;
    }

    Ok(())
}

pub fn diff_documents(old_doc: &Document, new_doc: &Document) -> Vec<HashDiff> {
    let max_len = old_doc.len().max(new_doc.len());
    let mut changes = Vec::new();

    for index in 0..max_len {
        match (old_doc.lines.get(index), new_doc.lines.get(index)) {
            (Some(old_line), Some(new_line)) if old_line.short_hash != new_line.short_hash => {
                changes.push(HashDiff {
                    line_no: index + 1,
                    kind: DiffKind::Changed,
                    old_hash: Some(format_short_hash(old_line.short_hash)),
                    new_hash: Some(format_short_hash(new_line.short_hash)),
                    content: new_line.content.clone(),
                });
            }
            (None, Some(new_line)) => {
                changes.push(HashDiff {
                    line_no: index + 1,
                    kind: DiffKind::Added,
                    old_hash: None,
                    new_hash: Some(format_short_hash(new_line.short_hash)),
                    content: new_line.content.clone(),
                });
            }
            (Some(old_line), None) => {
                changes.push(HashDiff {
                    line_no: index + 1,
                    kind: DiffKind::Removed,
                    old_hash: Some(format_short_hash(old_line.short_hash)),
                    new_hash: None,
                    content: old_line.content.clone(),
                });
            }
            _ => {}
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::{DiffKind, diff_documents, watch_file};
    use crate::document::Document;
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    #[test]
    fn test_diff_indexes_change() {
        let old_doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let new_doc = Document::from_str(Path::new("demo.txt"), "alpha\ngamma\n").unwrap();

        let changes = diff_documents(&old_doc, &new_doc);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].line_no, 2);
        assert_eq!(changes[0].kind, DiffKind::Changed);
    }

    #[test]
    fn test_diff_indexes_add_line() {
        let old_doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let new_doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let changes = diff_documents(&old_doc, &new_doc);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].line_no, 2);
        assert_eq!(changes[0].kind, DiffKind::Added);
    }

    #[test]
    fn test_diff_indexes_remove_line() {
        let old_doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let new_doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        let changes = diff_documents(&old_doc, &new_doc);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].line_no, 2);
        assert_eq!(changes[0].kind, DiffKind::Removed);
    }

    #[test]
    fn test_diff_indexes_no_change() {
        let old_doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let new_doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        assert!(diff_documents(&old_doc, &new_doc).is_empty());
    }

    #[test]
    fn test_watch_nonexistent_file_fails_immediately() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.txt");
        let mut out = Vec::new();

        let error = watch_file(&missing, true, false, &mut out).unwrap_err();

        assert!(matches!(error, crate::error::LinehashError::Io(_)));
    }

    #[cfg_attr(target_os = "macos", ignore = "flaky on macOS CI")]
    #[test]
    fn test_watch_detects_file_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, "alpha\n").unwrap();

        let watch_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let watch_handle = thread::spawn(move || {
            let mut out = Vec::new();
            let result = watch_file(&watch_path, true, false, &mut out);
            tx.send((result, out)).unwrap();
        });

        thread::sleep(Duration::from_millis(300));
        fs::write(&path, "beta\n").unwrap();

        let (result, out) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watch should finish after first change");
        result.unwrap();
        watch_handle.join().unwrap();
        let rendered = String::from_utf8(out).unwrap();

        assert!(rendered.contains("Watching"));
        assert!(rendered.contains("line 1 Changed") || rendered.contains("line 1 changed"));
        assert!(rendered.contains("beta"));
        assert!(rendered.contains("New index: 1 lines, 1 hash changes"));
    }

    #[cfg_attr(target_os = "macos", ignore = "flaky on macOS CI")]
    #[test]
    fn test_watch_once_exits_after_first_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, "alpha\n").unwrap();

        let watch_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let watch_handle = thread::spawn(move || {
            let start = Instant::now();
            let mut out = Vec::new();
            let result = watch_file(&watch_path, true, false, &mut out);
            tx.send((result, out, start.elapsed())).unwrap();
        });

        thread::sleep(Duration::from_millis(300));
        fs::write(&path, "beta\n").unwrap();
        thread::sleep(Duration::from_millis(100));
        fs::write(&path, "gamma\n").unwrap();

        let (result, _out, elapsed) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watch should exit after first change");
        result.unwrap();
        watch_handle.join().unwrap();

        assert!(elapsed < Duration::from_secs(2));
    }

    #[cfg_attr(target_os = "macos", ignore = "flaky on macOS CI")]
    #[test]
    fn test_json_output_parses() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, "alpha\n").unwrap();

        let watch_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let watch_handle = thread::spawn(move || {
            let mut out = Vec::new();
            let result = watch_file(&watch_path, true, true, &mut out);
            tx.send((result, out)).unwrap();
        });

        thread::sleep(Duration::from_millis(300));
        fs::write(&path, "beta\n").unwrap();

        let (result, out) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watch should emit JSON after first change");
        result.unwrap();
        watch_handle.join().unwrap();

        let rendered = String::from_utf8(out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(rendered.trim()).unwrap();
        assert_eq!(parsed["event"], "changed");
        assert_eq!(parsed["path"], path.display().to_string());
        assert_eq!(parsed["changes"][0]["line_no"], 1);
    }
}
