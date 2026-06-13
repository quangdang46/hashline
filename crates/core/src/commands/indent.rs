use std::io::Write;

use crate::anchor::{parse_range, resolve_range, try_parse_line_anchor, };
use crate::cli::IndentCmd;
use crate::commands::common::{atomic_write, atomic_write_document, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash_cache::discover_sidecar_root;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: IndentCmd,
) -> Result<(), HashlineError> {
    let root = discover_sidecar_root(&cmd.file);

    if !cmd.range.is_empty() && !cmd.range.contains("..") && !cmd.receipt && cmd.audit_log.is_none()
        && cmd.expect_mtime.is_none() && cmd.expect_inode.is_none() && !cmd.dry_run && !cmd.receipt
    {
        use crate::anchor::try_parse_line_anchor;
        let parts: Vec<&str> = cmd.range.split("..").collect();
        if let Some(first_anchor) = parts.first().and_then(|a| try_parse_line_anchor(a)) {
            let (l1, h1) = first_anchor;
            let (l2, h2) = if let Some(end) = parts.get(1).and_then(|a| try_parse_line_anchor(a)) { end } else { (l1, h1) };
            let amt: isize = cmd.amount.trim_start_matches('+').parse().unwrap_or(0);
            let content = crate::commands::fast_edit::read_file(&cmd.file)?;
            let nc = crate::commands::fast_edit::fast_indent_lines(&content, l1, l2, h1, amt)?;
            crate::commands::fast_edit::atomic_write(&cmd.file, &nc)?;
            if let Ok(doc) = crate::document::Document::from_str(&cmd.file, &nc) { ctx.modified_doc = Some(doc); }
            match ctx.output_mode() {
                crate::context::OutputMode::Pretty => {
                    let by = cmd.amount.trim_start_matches('+');
                    let msg = if l1 == l2 { format!("Indented line {} by {} spaces.", l1 + 1, by) }
                                    else { format!("Indented lines {}-{} by {} spaces.", l1 + 1, l2 + 1, by) };
                    crate::output::write_success_line(ctx, &msg).map_err(HashlineError::from)?;
                }
                _ => {}
            }
            return Ok(());
        }
    }

    let mut doc = Document::load_with_hash_cache(&cmd.file, &root)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());
    let range = parse_range(&cmd.range)?;
    let index = doc.build_index();
    let (start, end) = resolve_range(&range, &doc, &index)?;
    let change = parse_indent_change(&cmd.amount)?;
    validate_range_style(&doc, start.index, end.index)?;

    let mut changes = Vec::new();
    for idx in start.index..=end.index {
        let before = doc.lines[idx].content.to_string();
        let after = apply_indent(&before, change, idx + 1)?;
        doc.lines[idx].content = Box::from(after.as_str());
        changes.push(LineChange {
            line_no: idx + 1,
            kind: ChangeKind::Modified,
            before: Some(before),
            after: Some(after),
        });
    }

    for idx in start.index..=end.index {
        let line = &mut doc.lines[idx];
        line.short_hash = crate::hash::short_from_full(crate::hash::full_hash(&line.content));
    }

    let summary = IndentSummary {
        start_line: start.line_no,
        end_line: end.line_no,
        change,
        changes,
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
        let receipt = receipt::build_receipt(
            "indent",
            &cmd.file,
            summary.changes.clone(),
            before_bytes
                .as_deref()
                .expect("before bytes should exist when receipt is needed"),
            after_bytes
                .as_deref()
                .expect("after bytes should exist when receipt is needed"),
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

    fn dry_run_summary(&self) -> String {
        match self.change {
            IndentChange::Indent(amount) => format!(
                "Would indent lines {}-{} by {} spaces.",
                self.start_line, self.end_line, amount
            ),
            IndentChange::Dedent(amount) => format!(
                "Would dedent lines {}-{} by {} spaces.",
                self.start_line, self.end_line, amount
            ),
        }
    }
}

fn parse_indent_change(raw: &str) -> Result<IndentChange, HashlineError> {
    if raw.len() < 2 {
        return Err(HashlineError::InvalidIndentAmount { amount: raw.into() });
    }
    let (sign, amount) = raw.split_at(1);
    let parsed = amount
        .parse::<usize>()
        .ok()
        .filter(|amount| *amount > 0)
        .ok_or_else(|| HashlineError::InvalidIndentAmount { amount: raw.into() })?;
    match sign {
        "+" => Ok(IndentChange::Indent(parsed)),
        "-" => Ok(IndentChange::Dedent(parsed)),
        _ => Err(HashlineError::InvalidIndentAmount { amount: raw.into() }),
    }
}

fn validate_range_style(doc: &Document, start: usize, end: usize) -> Result<(), HashlineError> {
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
            return Err(HashlineError::MixedIndentation { line_no: idx + 1 });
        }
    }
    Ok(())
}

fn apply_indent(line: &str, change: IndentChange, line_no: usize) -> Result<String, HashlineError> {
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
                return Err(HashlineError::IndentUnderflow {
                    line_no,
                    amount,
                    available: available_tabs,
                    kind: "tabs",
                });
            }
            if available_spaces < amount {
                return Err(HashlineError::IndentUnderflow {
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
    file: &std::path::Path,
    summary: &IndentSummary,
) -> Result<(), HashlineError> {
    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            // PR-D: emit a compact mutation receipt instead of dumping the
            // entire proposed document.
            let receipt = receipt::build_dry_run_receipt(
                "indent",
                file,
                summary.dry_run_summary(),
                summary.changes.clone(),
            );
            receipt::write_dry_run_receipt(ctx, &receipt)
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
            output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)
        }
    }
}
