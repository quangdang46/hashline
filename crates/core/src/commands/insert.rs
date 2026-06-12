use std::io::Write;

use crate::anchor::{parse_anchor, resolve};
use crate::cli::InsertCmd;
use crate::commands::common::{
    atomic_write, atomic_write_document, check_guard, interpret_escapes,
};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash_cache::discover_sidecar_root;
use crate::mutation::insert_line;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: InsertCmd,
) -> Result<(), HashlineError> {
    let root = discover_sidecar_root(&cmd.file);
    let mut doc = Document::load_with_hash_cache(&cmd.file, &root)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());
    let content = if cmd.interpret_escapes {
        interpret_escapes(&cmd.content)
    } else {
        cmd.content
    };
    let index = doc.build_index();
    let anchor = parse_anchor(&cmd.anchor)?;
    let resolved = resolve(&anchor, &doc, &index)?;
    let insert_at = if cmd.before {
        resolved.index
    } else {
        resolved.index + 1
    };
    insert_line(&mut doc, insert_at, &content)?;

    let summary = InsertSummary {
        anchor_line: resolved.line_no,
        inserted_line: insert_at + 1,
        content,
        before: cmd.before,
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

    // Seed the session cache with the post-mutation document.
    ctx.modified_doc = Some(doc.clone());

    if needs_receipt {
        let after_bytes = after_bytes
            .as_deref()
            .expect("after bytes should exist when receipt is needed");
        let receipt = receipt::build_receipt(
            "insert",
            &cmd.file,
            summary.line_changes(),
            before_bytes
                .as_deref()
                .expect("before bytes should exist when receipt is needed"),
            after_bytes,
        );

        if let Some(log_path) = &cmd.audit_log {
            if let Err(error) = receipt::append_to_audit_log(&receipt, log_path) {
                receipt::write_audit_warning(ctx, log_path, &error).map_err(HashlineError::from)?;
            }
        }

        if cmd.receipt {
            return receipt::write_receipt(ctx, &receipt);
        }
    }

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => Ok(()),
        OutputMode::Pretty => {
            output::write_success_line(ctx, &summary.success_message()).map_err(HashlineError::from)
        }
    }
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &std::path::Path,
    summary: &InsertSummary,
) -> Result<(), HashlineError> {
    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            // PR-D: emit a compact mutation receipt instead of the proposed document.
            let receipt = receipt::build_dry_run_receipt(
                "insert",
                file,
                summary.dry_run_summary(),
                summary.line_changes(),
            );
            receipt::write_dry_run_receipt(ctx, &receipt)
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
            output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)
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

    fn dry_run_summary(&self) -> String {
        let relation = if self.before { "before" } else { "after" };
        format!(
            "Would insert line {} {relation} line {}.",
            self.inserted_line, self.anchor_line
        )
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
