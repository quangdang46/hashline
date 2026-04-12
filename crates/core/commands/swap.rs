use std::io::Write;

use crate::anchor::{parse_anchor, resolve};
use crate::cli::SwapCmd;
use crate::commands::common::{atomic_write, check_guard};
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::mutation::swap_lines;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: SwapCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());
    let index = doc.build_index();

    let anchor_a = parse_anchor(&cmd.anchor_a)?;
    let anchor_b = parse_anchor(&cmd.anchor_b)?;
    let resolved_a = resolve(&anchor_a, &doc, &index)?;
    let resolved_b = resolve(&anchor_b, &doc, &index)?;

    if resolved_a.index == resolved_b.index {
        return Err(LinehashError::PatchFailed {
            op_index: 0,
            reason: "source and target must resolve to different lines".to_owned(),
        });
    }

    let line_a = doc.lines[resolved_a.index].content.clone();
    let line_b = doc.lines[resolved_b.index].content.clone();
    swap_lines(&mut doc, resolved_a.index, resolved_b.index)?;

    let summary = SwapSummary {
        line_a_no: resolved_a.line_no,
        line_b_no: resolved_b.line_no,
        line_a_content: line_a,
        line_b_content: line_b,
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &summary);
    }

    let after_bytes = doc.render();
    atomic_write(&cmd.file, &after_bytes)?;

    if needs_receipt {
        let receipt = receipt::build_receipt(
            "swap",
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
    summary: &SwapSummary,
) -> Result<(), LinehashError> {
    output::write_success_line(ctx, &summary.preview_message())?;
    output::write_success_line(
        ctx,
        &format!("  {} ↔ {:?}", summary.line_a_no, summary.line_b_content),
    )?;
    output::write_success_line(
        ctx,
        &format!("  {} ↔ {:?}", summary.line_b_no, summary.line_a_content),
    )?;
    output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
}

struct SwapSummary {
    line_a_no: usize,
    line_b_no: usize,
    line_a_content: String,
    line_b_content: String,
}

impl SwapSummary {
    fn success_message(&self) -> String {
        format!("Swapped lines {} and {}.", self.line_a_no, self.line_b_no)
    }

    fn preview_message(&self) -> String {
        format!(
            "Would swap line {} with line {}:",
            self.line_a_no, self.line_b_no
        )
    }

    fn line_changes(&self) -> Vec<LineChange> {
        vec![
            LineChange {
                line_no: self.line_a_no,
                kind: ChangeKind::Modified,
                before: Some(self.line_a_content.clone()),
                after: Some(self.line_b_content.clone()),
            },
            LineChange {
                line_no: self.line_b_no,
                kind: ChangeKind::Modified,
                before: Some(self.line_b_content.clone()),
                after: Some(self.line_a_content.clone()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::cli::SwapCmd;
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
    fn swaps_two_lines() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\ndelta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            SwapCmd {
                file: path.clone(),
                anchor_a: line_anchor("alpha\nbeta\ngamma\ndelta\n", 2),
                anchor_b: line_anchor("alpha\nbeta\ngamma\ndelta\n", 4),
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
            "alpha\ndelta\ngamma\nbeta\n"
        );
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Swapped lines 2 and 4.\n"
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn dry_run_reports_swap_without_writing_file() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\ndelta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            SwapCmd {
                file: path.clone(),
                anchor_a: line_anchor("alpha\nbeta\ngamma\ndelta\n", 1),
                anchor_b: line_anchor("alpha\nbeta\ngamma\ndelta\n", 3),
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
            "Would swap line 1 with line 3:\n  1 ↔ \"gamma\"\n  3 ↔ \"alpha\"\nNo file was written.\n"
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn rejects_same_line_swap() {
        let (_dir, path) = temp_file("alpha\nbeta\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        let error = run(
            &mut ctx,
            SwapCmd {
                file: path,
                anchor_a: line_anchor("alpha\nbeta\n", 2),
                anchor_b: line_anchor("alpha\nbeta\n", 2),
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
    fn emits_receipt_for_swap() {
        let (_dir, path) = temp_file("alpha\nbeta\ngamma\n");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = CommandContext::new(&mut stdout, &mut stderr, OutputMode::Pretty);

        run(
            &mut ctx,
            SwapCmd {
                file: path,
                anchor_a: line_anchor("alpha\nbeta\ngamma\n", 1),
                anchor_b: line_anchor("alpha\nbeta\ngamma\n", 3),
                dry_run: false,
                receipt: true,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
            },
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(parsed["op"], "swap");
        assert_eq!(parsed["changes"][0]["kind"], "Modified");
        assert_eq!(parsed["changes"][0]["after"], "gamma");
        assert_eq!(parsed["changes"][1]["after"], "alpha");
        assert!(stderr.is_empty());
    }
}
