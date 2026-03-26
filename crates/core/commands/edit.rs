use std::io::Write;

use crate::anchor::{parse_anchor, parse_range, resolve_range, resolve_without_index};
use crate::cli::EditCmd;
use crate::commands::common::{atomic_write, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::mutation::{replace_line, replace_range_with_line};
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: EditCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());

    let summary = match cmd.anchor.contains("..") {
        true => {
            let range = parse_range(&cmd.anchor)?;
            let index = doc.build_index();
            let (start, end) = resolve_range(&range, &doc, &index)?;
            let before = doc.lines[start.index..=end.index]
                .iter()
                .map(|line| line.content.clone())
                .collect::<Vec<_>>();
            replace_range_with_line(&mut doc, start.index, end.index, &cmd.content)?;
            EditSummary::Range {
                start_line: start.line_no,
                end_line: end.line_no,
                before,
                after: cmd.content,
            }
        }
        false => {
            let anchor = parse_anchor(&cmd.anchor)?;
            let resolved = resolve_without_index(&anchor, &doc)?;
            let before = doc.lines[resolved.index].content.clone();
            replace_line(&mut doc, resolved.index, &cmd.content)?;
            EditSummary::Single {
                line_no: resolved.line_no,
                before,
                after: cmd.content,
            }
        }
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &doc, &summary);
    }

    let after_bytes = doc.render();
    atomic_write(&cmd.file, &after_bytes)?;

    if needs_receipt {
        let receipt = receipt::build_receipt(
            "edit",
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

    match ctx.output_mode() {
        OutputMode::Json => Ok(()),
        OutputMode::Pretty => {
            output::write_success_line(ctx, &summary.success_message()).map_err(LinehashError::from)
        }
    }
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    doc: &Document,
    summary: &EditSummary,
) -> Result<(), LinehashError> {
    match ctx.output_mode() {
        OutputMode::Json => output::print_read_json(ctx.stdout(), doc).map_err(LinehashError::from),
        OutputMode::Pretty => {
            match summary {
                EditSummary::Single {
                    line_no,
                    before,
                    after,
                } => {
                    output::write_success_line(ctx, &format!("Would change line {line_no}:"))?;
                    output::write_success_line(ctx, &format!("  - {before:?}"))?;
                    output::write_success_line(ctx, &format!("  + {after:?}"))?;
                }
                EditSummary::Range {
                    start_line,
                    end_line,
                    before,
                    after,
                } => {
                    output::write_success_line(
                        ctx,
                        &format!("Would change lines {start_line}-{end_line}:"),
                    )?;
                    for line in before {
                        output::write_success_line(ctx, &format!("  - {line:?}"))?;
                    }
                    output::write_success_line(ctx, &format!("  + {after:?}"))?;
                }
            }
            output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
        }
    }
}

enum EditSummary {
    Single {
        line_no: usize,
        before: String,
        after: String,
    },
    Range {
        start_line: usize,
        end_line: usize,
        before: Vec<String>,
        after: String,
    },
}

impl EditSummary {
    fn success_message(&self) -> String {
        match self {
            EditSummary::Single { line_no, .. } => format!("Edited line {line_no}."),
            EditSummary::Range {
                start_line,
                end_line,
                ..
            } => format!("Edited lines {start_line}-{end_line}."),
        }
    }

    fn line_changes(&self) -> Vec<LineChange> {
        match self {
            EditSummary::Single {
                line_no,
                before,
                after,
            } => vec![LineChange {
                line_no: *line_no,
                kind: ChangeKind::Modified,
                before: Some(before.clone()),
                after: Some(after.clone()),
            }],
            EditSummary::Range {
                start_line,
                before,
                after,
                ..
            } => {
                let mut changes = Vec::with_capacity(before.len());
                if let Some(first) = before.first() {
                    changes.push(LineChange {
                        line_no: *start_line,
                        kind: ChangeKind::Modified,
                        before: Some(first.clone()),
                        after: Some(after.clone()),
                    });
                }
                for (offset, removed) in before.iter().enumerate().skip(1) {
                    changes.push(LineChange {
                        line_no: *start_line + offset,
                        kind: ChangeKind::Deleted,
                        before: Some(removed.clone()),
                        after: None,
                    });
                }
                changes
            }
        }
    }
}
