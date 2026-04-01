use std::io::Write;

use crate::anchor::{parse_anchor, resolve};
use crate::cli::InsertCmd;
use crate::commands::common::{atomic_write, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::mutation::insert_line;
use crate::orchestration::read_payload;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: InsertCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());
    let index = doc.build_index();
    let anchor = parse_anchor(&cmd.anchor)?;
    let resolved = resolve(&anchor, &doc, &index)?;
    let insert_at = if cmd.before {
        resolved.index
    } else {
        resolved.index + 1
    };
    insert_line(&mut doc, insert_at, &cmd.content)?;

    let summary = InsertSummary {
        anchor_line: resolved.line_no,
        inserted_line: insert_at + 1,
        content: cmd.content,
        before: cmd.before,
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &doc, &summary);
    }

    let after_bytes = doc.render();
    atomic_write(&cmd.file, &after_bytes)?;

    if needs_receipt {
        let receipt = receipt::build_receipt(
            "insert",
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
    summary: &InsertSummary,
) -> Result<(), LinehashError> {
    match ctx.output_mode() {
        OutputMode::Json => {
            let payload = read_payload(doc, &[], 0)?;
            output::print_read_json(ctx.stdout(), &payload).map_err(LinehashError::from)
        }
        OutputMode::Pretty => {
            let relation = if summary.before { "before" } else { "after" };
            output::write_success_line(
                ctx,
                &format!(
                    "Would insert line {} {relation} line {}:",
                    summary.inserted_line, summary.anchor_line
                ),
            )?;
            output::write_success_line(ctx, &format!("  + {:?}", summary.content))?;
            output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
        }
    }
}

struct InsertSummary {
    anchor_line: usize,
    inserted_line: usize,
    content: String,
    before: bool,
}

impl InsertSummary {
    fn success_message(&self) -> String {
        format!("Inserted line {}.", self.inserted_line)
    }

    fn line_changes(&self) -> Vec<LineChange> {
        vec![LineChange {
            line_no: self.inserted_line,
            kind: ChangeKind::Inserted,
            before: None,
            after: Some(self.content.clone()),
        }]
    }
}
