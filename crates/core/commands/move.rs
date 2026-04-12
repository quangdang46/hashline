use std::io::Write;

use crate::anchor::{parse_anchor, resolve};
use crate::cli::{MoveCmd, MoveDirection};
use crate::commands::common::{atomic_write, check_guard};
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::mutation::move_line;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: MoveCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());
    let index = doc.build_index();

    let source_anchor = parse_anchor(&cmd.anchor)?;
    let target_anchor = parse_anchor(&cmd.target)?;
    let source = resolve(&source_anchor, &doc, &index)?;
    let target = resolve(&target_anchor, &doc, &index)?;
    let moved_content = doc.lines[source.index].content.clone();
    let place_before = matches!(cmd.direction, MoveDirection::Before);
    let inserted_index = move_line(&mut doc, source.index, target.index, place_before)?;

    let summary = MoveSummary {
        source_line: source.line_no,
        target_line: target.line_no,
        inserted_line: inserted_index + 1,
        moved_content,
        direction: cmd.direction,
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &summary);
    }

    let after_bytes = doc.render();
    atomic_write(&cmd.file, &after_bytes)?;

    if needs_receipt {
        let receipt = receipt::build_receipt(
            "move",
            &cmd.file,
            summary.line_changes(),
            before_bytes
                .as_deref()
                .expect("before bytes should exist when receipt is needed"),
            &after_bytes,
        );

        if let Some(log_path) = &cmd.audit_log {
            if let Err(error) = receipt::append_to_audit_log(&receipt, log_path) {
                receipt::write_audit_warning(ctx, log_path, &error).map_err(LinehashError::from)?;
            }
        }

        if cmd.receipt {
            return receipt::write_receipt(ctx, &receipt);
        }
    }

    output::write_success_line(ctx, &summary.success_message()).map_err(LinehashError::from)
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    summary: &MoveSummary,
) -> Result<(), LinehashError> {
    output::write_success_line(ctx, &summary.preview_message())?;
    output::write_success_line(ctx, &format!("  ~ {:?}", summary.moved_content))?;
    output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
}

struct MoveSummary {
    source_line: usize,
    target_line: usize,
    inserted_line: usize,
    moved_content: String,
    direction: MoveDirection,
}

impl MoveSummary {
    fn success_message(&self) -> String {
        format!(
            "Moved line {} to line {}.",
            self.source_line, self.inserted_line
        )
    }

    fn preview_message(&self) -> String {
        format!(
            "Would move line {} {} line {} to line {}:",
            self.source_line,
            self.direction_word(),
            self.target_line,
            self.inserted_line
        )
    }

    fn direction_word(&self) -> &'static str {
        match self.direction {
            MoveDirection::After => "after",
            MoveDirection::Before => "before",
        }
    }

    fn line_changes(&self) -> Vec<LineChange> {
        vec![
            LineChange {
                line_no: self.source_line,
                kind: ChangeKind::Deleted,
                before: Some(self.moved_content.clone()),
                after: None,
            },
            LineChange {
                line_no: self.inserted_line,
                kind: ChangeKind::Inserted,
                before: None,
                after: Some(self.moved_content.clone()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::cli::{MoveCmd, MoveDirection};
    use crate::context::{CommandContext, OutputMode};
    use crate::document::Document;
    use crate::error::LinehashError;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn temp_file(content: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn line_anchor(content: &str, line_no: usize) -> String {
        let doc = Document::from_str(Path::new("demo.txt"), content).unwrap();
        let line = &doc.lines[line_no - 1];
        format!(
            "{}:{}",
            line_no,
            crate::document::format_short_hash(line.short_hash)
        )
    }

    #[test]
    fn moves_line_after_target() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\ndelta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            MoveCmd {
                file: path.clone(),
                anchor: line_anchor("alpha\nbeta\ngamma\ndelta\n", 2),
                direction: MoveDirection::After,
                target: line_anchor("alpha\nbeta\ngamma\ndelta\n", 4),
                dry_run: false,
                receipt: false,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "alpha\ngamma\ndelta\nbeta\n"
        );
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Moved line 2 to line 4.\n"
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn moves_line_before_target_in_dry_run() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\ndelta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            MoveCmd {
                file: path.clone(),
                anchor: line_anchor("alpha\nbeta\ngamma\ndelta\n", 4),
                direction: MoveDirection::Before,
                target: line_anchor("alpha\nbeta\ngamma\ndelta\n", 2),
                dry_run: true,
                receipt: false,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "alpha\nbeta\ngamma\ndelta\n"
        );
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Would move line 4 before line 2 to line 2:\n  ~ \"delta\"\nNo file was written.\n"
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn rejects_same_line_move() {
        let (_dir, path) = temp_file("alpha\nbeta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        let error = run(
            &mut ctx,
            MoveCmd {
                file: path,
                anchor: line_anchor("alpha\nbeta\n", 2),
                direction: MoveDirection::Before,
                target: line_anchor("alpha\nbeta\n", 2),
                dry_run: false,
                receipt: false,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            LinehashError::PatchFailed { op_index: 0, .. }
        ));
        assert!(
            error
                .to_string()
                .contains("source and target must resolve to different lines")
        );
    }

    #[test]
    fn emits_receipt_for_move() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            MoveCmd {
                file: path,
                anchor: line_anchor("alpha\nbeta\ngamma\n", 1),
                direction: MoveDirection::After,
                target: line_anchor("alpha\nbeta\ngamma\n", 3),
                dry_run: false,
                receipt: true,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
            },
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["op"], "move");
        assert_eq!(parsed["changes"][0]["kind"], "Deleted");
        assert_eq!(parsed["changes"][1]["kind"], "Inserted");
        assert_eq!(parsed["changes"][1]["after"], "alpha");
        assert!(stderr.is_empty());
    }
}
