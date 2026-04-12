use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::hash;
use crate::output;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Receipt {
    pub op: String,
    pub file: String,
    pub timestamp: i64,
    pub changes: Vec<LineChange>,
    pub file_hash_before: u32,
    pub file_hash_after: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LineChange {
    pub line_no: usize,
    pub kind: ChangeKind,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum ChangeKind {
    Modified,
    Inserted,
    Deleted,
}

pub fn build_receipt(
    op: &str,
    file: &Path,
    changes: Vec<LineChange>,
    bytes_before: &[u8],
    bytes_after: &[u8],
) -> Receipt {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    Receipt {
        op: op.to_owned(),
        file: file.display().to_string(),
        timestamp,
        changes,
        file_hash_before: hash::full_hash_bytes(bytes_before),
        file_hash_after: hash::full_hash_bytes(bytes_after),
    }
}

pub fn write_receipt<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    receipt: &Receipt,
) -> Result<(), LinehashError> {
    output::write_json_success(ctx, receipt).map_err(LinehashError::from)
}

pub fn append_to_audit_log(receipt: &Receipt, log_path: &Path) -> Result<(), LinehashError> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    serde_json::to_writer(&mut file, receipt)?;
    writeln!(file)?;
    file.flush()?;
    Ok(())
}

pub fn write_audit_warning<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    log_path: &Path,
    error: &LinehashError,
) -> io::Result<()> {
    writeln!(
        ctx.stderr(),
        "Warning: wrote file but failed to append audit log '{}': {error}",
        log_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{ChangeKind, LineChange, append_to_audit_log, build_receipt};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_build_receipt_populates_expected_fields() {
        let receipt = build_receipt(
            "edit",
            Path::new("demo.txt"),
            vec![LineChange {
                line_no: 2,
                kind: ChangeKind::Modified,
                before: Some("beta".into()),
                after: Some("gamma".into()),
            }],
            b"alpha\nbeta\n",
            b"alpha\ngamma\n",
        );

        assert_eq!(receipt.op, "edit");
        assert_eq!(receipt.file, "demo.txt");
        assert_eq!(receipt.changes.len(), 1);
        assert!(receipt.timestamp >= 0);
        assert_ne!(receipt.file_hash_before, receipt.file_hash_after);
    }

    #[test]
    fn test_append_creates_file_if_not_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        let receipt = build_receipt("edit", Path::new("demo.txt"), vec![], b"a", b"b");

        append_to_audit_log(&receipt, &path).unwrap();

        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        let parsed: serde_json::Value =
            serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        assert_eq!(parsed["op"], "edit");
    }

    #[test]
    fn test_append_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/logs/audit.jsonl");
        let receipt = build_receipt("insert", Path::new("demo.txt"), vec![], b"a", b"ab");

        append_to_audit_log(&receipt, &path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_append_two_receipts_is_valid_jsonl() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        let first = build_receipt("edit", Path::new("demo.txt"), vec![], b"a", b"b");
        let second = build_receipt("delete", Path::new("demo.txt"), vec![], b"b", b"");

        append_to_audit_log(&first, &path).unwrap();
        append_to_audit_log(&second, &path).unwrap();

        let contents = fs::read_to_string(path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first_parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second_parsed: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first_parsed["op"], "edit");
        assert_eq!(second_parsed["op"], "delete");
    }

    #[test]
    fn test_append_does_not_truncate_existing_log() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        fs::write(&path, "{\"op\":\"existing\"}\n").unwrap();
        let receipt = build_receipt("edit", Path::new("demo.txt"), vec![], b"a", b"b");

        append_to_audit_log(&receipt, &path).unwrap();

        let contents = fs::read_to_string(path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first_parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first_parsed["op"], "existing");
    }
}
