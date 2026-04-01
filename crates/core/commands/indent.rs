use std::io::Write;

use crate::anchor::{parse_range, resolve_range};
use crate::cli::IndentCmd;
use crate::commands::common::{atomic_write, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::read_payload;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: IndentCmd,
) -> Result<(), LinehashError> {
    let mut doc = Document::load(&cmd.file)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let before_bytes = doc.render();
    let range = parse_range(&cmd.range)?;
    let index = doc.build_index();
    let (start, end) = resolve_range(&range, &doc, &index)?;
    let change = parse_indent_change(&cmd.amount)?;
    validate_range_style(&doc, start.index, end.index)?;

    let mut changes = Vec::new();
    for idx in start.index..=end.index {
        let before = doc.lines[idx].content.clone();
        let after = apply_indent(&before, change, idx + 1)?;
        doc.lines[idx].content = after.clone();
        changes.push(LineChange {
            line_no: idx + 1,
            kind: ChangeKind::Modified,
            before: Some(before),
            after: Some(after),
        });
    }

    for idx in start.index..=end.index {
        let line = &mut doc.lines[idx];
        line.full_hash = crate::hash::full_hash(&line.content);
        line.short_hash = crate::hash::short_from_full(line.full_hash);
    }

    let summary = IndentSummary {
        start_line: start.line_no,
        end_line: end.line_no,
        change,
        changes,
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &doc, &summary);
    }

    let after_bytes = doc.render();
    atomic_write(&cmd.file, &after_bytes)?;

    let receipt = receipt::build_receipt(
        "indent",
        &cmd.file,
        summary.changes.clone(),
        &before_bytes,
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

    match ctx.output_mode() {
        OutputMode::Json => Ok(()),
        OutputMode::Pretty => {
            output::write_success_line(ctx, &summary.success_message()).map_err(LinehashError::from)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum IndentChange {
    Indent(usize),
    Dedent(usize),
}

#[derive(Clone, Debug)]
struct IndentSummary {
    start_line: usize,
    end_line: usize,
    change: IndentChange,
    changes: Vec<LineChange>,
}

impl IndentSummary {
    fn success_message(&self) -> String {
        match self.change {
            IndentChange::Indent(amount) => format!(
                "Indented lines {}-{} by {} spaces.",
                self.start_line, self.end_line, amount
            ),
            IndentChange::Dedent(amount) => format!(
                "Dedented lines {}-{} by {} spaces.",
                self.start_line, self.end_line, amount
            ),
        }
    }
}

fn parse_indent_change(raw: &str) -> Result<IndentChange, LinehashError> {
    if raw.len() < 2 {
        return Err(LinehashError::InvalidIndentAmount { amount: raw.into() });
    }
    let (sign, amount) = raw.split_at(1);
    let parsed = amount
        .parse::<usize>()
        .ok()
        .filter(|amount| *amount > 0)
        .ok_or_else(|| LinehashError::InvalidIndentAmount { amount: raw.into() })?;
    match sign {
        "+" => Ok(IndentChange::Indent(parsed)),
        "-" => Ok(IndentChange::Dedent(parsed)),
        _ => Err(LinehashError::InvalidIndentAmount { amount: raw.into() }),
    }
}

fn validate_range_style(doc: &Document, start: usize, end: usize) -> Result<(), LinehashError> {
    let mut saw_spaces = false;
    let mut saw_tabs = false;
    for idx in start..=end {
        let line = &doc.lines[idx].content;
        match line.chars().next() {
            Some(' ') => saw_spaces = true,
            Some('\t') => saw_tabs = true,
            _ => {}
        }
        if saw_spaces && saw_tabs {
            return Err(LinehashError::MixedIndentation { line_no: idx + 1 });
        }
    }
    Ok(())
}

fn apply_indent(line: &str, change: IndentChange, line_no: usize) -> Result<String, LinehashError> {
    match change {
        IndentChange::Indent(amount) => Ok(format!("{}{}", " ".repeat(amount), line)),
        IndentChange::Dedent(amount) => {
            let mut available_spaces = 0;
            let mut available_tabs = 0;
            for ch in line.chars() {
                match ch {
                    ' ' => available_spaces += 1,
                    '\t' => available_tabs += 1,
                    _ => break,
                }
            }

            if available_tabs > 0 {
                return Err(LinehashError::IndentUnderflow {
                    line_no,
                    amount,
                    available: available_tabs,
                    kind: "tabs",
                });
            }
            if available_spaces < amount {
                return Err(LinehashError::IndentUnderflow {
                    line_no,
                    amount,
                    available: available_spaces,
                    kind: "spaces",
                });
            }
            Ok(line[amount..].to_owned())
        }
    }
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    doc: &Document,
    summary: &IndentSummary,
) -> Result<(), LinehashError> {
    match ctx.output_mode() {
        OutputMode::Json => {
            let payload = read_payload(doc, &[], 0)?;
            output::print_read_json(ctx.stdout(), &payload).map_err(LinehashError::from)
        }
        OutputMode::Pretty => {
            let change = match summary.change {
                IndentChange::Indent(amount) => format!(
                    "indent lines {}-{} by {} spaces",
                    summary.start_line, summary.end_line, amount
                ),
                IndentChange::Dedent(amount) => format!(
                    "dedent lines {}-{} by {} spaces",
                    summary.start_line, summary.end_line, amount
                ),
            };
            output::write_success_line(ctx, &format!("Would {change}:"))?;
            for change in &summary.changes {
                output::write_success_line(
                    ctx,
                    &format!(
                        "  {}: {:?} -> {:?}",
                        change.line_no,
                        change.before.as_deref().unwrap_or(""),
                        change.after.as_deref().unwrap_or("")
                    ),
                )?;
            }
            output::write_success_line(ctx, "No file was written.").map_err(LinehashError::from)
        }
    }
}
