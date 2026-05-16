use std::io::Write;

use crate::anchor::{looks_like_range_anchor, parse_anchor, parse_range, resolve, resolve_range};
use crate::cli::DeleteCmd;
use crate::commands::common::{atomic_write, atomic_write_document, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::mutation::{delete_line, delete_range};
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: DeleteCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());

    let summary = match looks_like_range_anchor(&cmd.anchor) {
        true => {
            let range = parse_range(&cmd.anchor)?;
            let index = doc.build_index();
            let (start, end) = resolve_range(&range, &doc, &index)?;
            let deleted = doc.lines[start.index..=end.index]
                .iter()
                .map(|line| line.content.clone())
                .collect::<Vec<_>>();
            delete_range(&mut doc, start.index, end.index)?;
            DeleteSummary::range(start.line_no, end.line_no, deleted)
        }
        false => {
            let index = doc.build_index();
            let anchor = parse_anchor(&cmd.anchor)?;
            let resolved = resolve(&anchor, &doc, &index)?;
            let deleted = doc.lines[resolved.index].content.clone();
            delete_line(&mut doc, resolved.index)?;
            DeleteSummary::single(resolved.line_no, deleted)
        }
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &cmd.file, &summary);
    }

    let after_bytes = if needs_receipt {
        let bytes = doc.render();
        atomic_write(&cmd.file, &bytes)?;
        Some(bytes)
    } else {
        atomic_write_document(&cmd.file, &doc)?;
        None
    };

    if needs_receipt {
        let before_bytes = before_bytes.as_deref().ok_or_else(|| {
            std::io::Error::other("before bytes should exist when receipt is needed")
        })?;
        let after_bytes = after_bytes.as_deref().ok_or_else(|| {
            std::io::Error::other("after bytes should exist when receipt is needed")
        })?;
        let receipt = receipt::build_receipt(
            "delete",
            &cmd.file,
            summary.line_changes(),
            before_bytes,
            after_bytes,
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

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => Ok(()),
        OutputMode::Pretty => {
            output::write_success_line(ctx, &summary.success_message()).map_err(LinehashError::from)
        }
    }
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &std::path::Path,
    summary: &DeleteSummary,
) -> Result<(), LinehashError> {
    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            // PR-D: emit a compact mutation receipt instead of the proposed document.
            let receipt = receipt::build_dry_run_receipt(
                "delete",
                file,
                summary.dry_run_summary(),
                summary.line_changes(),
            );
            receipt::write_dry_run_receipt(ctx, &receipt)
        }
        OutputMode::Pretty => match &summary.kind {
            DeleteSummaryKind::Single { line_no, deleted } => {
                output::write_success_line(ctx, &format!("Would delete line {line_no}:"))?;
                output::write_success_line(ctx, &format!("  - {deleted:?}"))?;
                output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
            }
            DeleteSummaryKind::Range {
                start_line,
                end_line,
                deleted,
            } => {
                output::write_success_line(
                    ctx,
                    &format!("Would delete lines {start_line}-{end_line}:"),
                )?;
                for line in deleted {
                    output::write_success_line(ctx, &format!("  - {line:?}"))?;
                }
                output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
            }
        },
    }
}

struct DeleteSummary {
    kind: DeleteSummaryKind,
}

enum DeleteSummaryKind {
    Single {
        line_no: usize,
        deleted: String,
    },
    Range {
        start_line: usize,
        end_line: usize,
        deleted: Vec<String>,
    },
}

impl DeleteSummary {
    fn single(line_no: usize, deleted: String) -> Self {
        Self {
            kind: DeleteSummaryKind::Single { line_no, deleted },
        }
    }

    fn range(start_line: usize, end_line: usize, deleted: Vec<String>) -> Self {
        Self {
            kind: DeleteSummaryKind::Range {
                start_line,
                end_line,
                deleted,
            },
        }
    }

    fn success_message(&self) -> String {
        match &self.kind {
            DeleteSummaryKind::Single { line_no, .. } => format!("Deleted line {line_no}."),
            DeleteSummaryKind::Range {
                start_line,
                end_line,
                ..
            } => format!("Deleted lines {start_line}-{end_line}."),
        }
    }

    fn dry_run_summary(&self) -> String {
        match &self.kind {
            DeleteSummaryKind::Single { line_no, .. } => format!("Would delete line {line_no}."),
            DeleteSummaryKind::Range {
                start_line,
                end_line,
                ..
            } => format!("Would delete lines {start_line}-{end_line}."),
        }
    }

    fn line_changes(&self) -> Vec<LineChange> {
        match &self.kind {
            DeleteSummaryKind::Single { line_no, deleted } => vec![LineChange {
                line_no: *line_no,
                kind: ChangeKind::Deleted,
                before: Some(deleted.clone()),
                after: None,
            }],
            DeleteSummaryKind::Range {
                start_line,
                deleted,
                ..
            } => deleted
                .iter()
                .enumerate()
                .map(|(offset, removed)| LineChange {
                    line_no: *start_line + offset,
                    kind: ChangeKind::Deleted,
                    before: Some(removed.clone()),
                    after: None,
                })
                .collect(),
        }
    }
}
