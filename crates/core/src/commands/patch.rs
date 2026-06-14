#![allow(clippy::redundant_pattern_matching, clippy::manual_filter, clippy::match_like_matches_macro, clippy::all, dead_code)]
use std::fs;
use std::io::{self, Read, Write};
use std::ops::RangeInclusive;

use serde::Deserialize;

use crate::anchor::{parse_anchor, parse_range, resolve, resolve_range};
use crate::cli::PatchCmd;
use crate::commands::common::{atomic_write, check_guard};
use crate::context::{CommandContext, OutputMode};
use crate::document::{Document, LineRecord};
use crate::error::HashlineError;
use crate::hash;
use crate::hash_cache::discover_sidecar_root;
use crate::mutation::validate_single_line_content;
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: PatchCmd,
) -> Result<(), HashlineError> {
    let patch = read_patch(&cmd.patch)?;
    validate_patch_target(&patch, &cmd.file)?;
    // Fast path: all ops with simple anchors -> multi-op string scanning
    if !cmd.dry_run && !cmd.receipt && cmd.audit_log.is_none()
        && cmd.expect_mtime.is_none() && cmd.expect_inode.is_none()
    {
        if patch.ops.len() == 1 {
            if let Some(anchor) = patch.ops.first().and_then(|op| match op {
                PatchOp::Edit(e) => Some(&e.anchor),
                PatchOp::Insert(i) => Some(&i.anchor),
                PatchOp::Delete(d) => Some(&d.anchor),
            }) {
                // Only use fast path if anchor resolves AND file content exists
                if crate::anchor::try_parse_line_anchor(anchor).is_some() && crate::anchor::try_parse_line_anchor(anchor).unwrap().0 < std::fs::read_to_string(&cmd.file).map(|c| c.lines().count()).unwrap_or(0) {
                    if let Ok(content) = std::fs::read_to_string(&cmd.file) {
                        let nlines = content.lines().count();
                        let (ln, _) = crate::anchor::try_parse_line_anchor(anchor).unwrap();
                        if ln < nlines {
                        }
                    }
                }
            }
        }
    }
    // Fast path: single Edit op with simple anchor
    if !cmd.dry_run && !cmd.receipt && cmd.audit_log.is_none()
        && cmd.expect_mtime.is_none() && cmd.expect_inode.is_none()
    {
    }
    validate_patch_target(&patch, &cmd.file)?;

    let original = Document::load_with_hash_cache(&cmd.file, &discover_sidecar_root(&cmd.file))?;
    check_guard(&original, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| original.render());
    let index = original.build_index();
    let plan = build_plan(&patch, &original, &index)?;
    let result = apply_plan(&original, &plan)?;

    if cmd.dry_run {
        return write_dry_run(ctx, &cmd.file, &result.summary, &result.changes);
    }

    let after_bytes = result.document.render();
    atomic_write(&cmd.file, &after_bytes)?;

    // Seed the session cache with the post-mutation document.
    ctx.modified_doc = Some(result.document.clone());

    if needs_receipt {
        let receipt = receipt::build_receipt(
            "patch",
            &cmd.file,
            result.changes.clone(),
            before_bytes
                .as_deref()
                .expect("before bytes should exist when receipt is needed"),
            &after_bytes,
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
        OutputMode::Pretty => output::write_success_line(ctx, &result.summary.success_message())
            .map_err(HashlineError::from),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchFile {
    file: Option<String>,
    ops: Vec<PatchOp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum PatchOp {
    Edit(EditOp),
    Insert(InsertOp),
    Delete(DeleteOp),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditOp {
    anchor: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct InsertOp {
    anchor: String,
    content: String,
    before: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteOp {
    anchor: String,
}

#[derive(Clone, Debug)]
enum PlannedOp {
    EditSingle {
        op_index: usize,
        line: usize,
        content: String,
        before: String,
    },
    EditRange {
        op_index: usize,
        range: RangeInclusive<usize>,
        content: String,
        before: Vec<String>,
    },
    Insert {
        boundary: usize,
        content: String,
        before: bool,
        anchor_line: usize,
    },
    Delete {
        op_index: usize,
        line: usize,
        deleted: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Occupancy {
    Edit,
    Delete,
}

#[derive(Clone, Debug)]
struct PatchResult {
    document: Document,
    summary: PatchSummary,
    changes: Vec<LineChange>,
}

#[derive(Clone, Debug)]
struct PatchSummary {
    op_count: usize,
    actions: Vec<String>,
    edit_count: usize,
    insert_count: usize,
    delete_count: usize,
}

impl PatchSummary {
    fn success_message(&self) -> String {
        format!(
            "Applied {} ops: {} edit{}, {} insert{}, {} delete{}.",
            self.op_count,
            self.edit_count,
            plural_suffix(self.edit_count),
            self.insert_count,
            plural_suffix(self.insert_count),
            self.delete_count,
            plural_suffix(self.delete_count)
        )
    }
}


struct FastOp { line: usize, op: PatchOp, op_index: usize }
fn run_fast_patch<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &std::path::Path,
    patch: &PatchFile,
) -> Result<(), HashlineError> {
    use crate::anchor::try_parse_line_anchor;

    // Read file content
    let mut content = crate::fast::read_file(file)?;
    let mut changes: Vec<(usize, String)> = Vec::new();

    // Collect all ops that can be resolved via try_parse_line_anchor
    let mut fast_ops: Vec<FastOp> = Vec::new();

    for (idx, op) in patch.ops.iter().enumerate() {
        let anchor = match op {
            PatchOp::Edit(e) => Some(&e.anchor),
            PatchOp::Insert(i) => Some(&i.anchor),
            PatchOp::Delete(d) => Some(&d.anchor),
        };
        // _ = anchor to suppress warning
        if let Some(anchor_str) = anchor {
            if let Some((line_no, _hash)) = try_parse_line_anchor(anchor_str) {
                fast_ops.push(FastOp { line: line_no, op: op.clone(), op_index: idx });
            }
        }
    }

    // If any op can't be resolved, fall through
    if fast_ops.len() != patch.ops.len() {
        return Err(HashlineError::PatchFailed { op_index: 0, reason: "complex anchors not supported in fast path".into() });
    }

    // Sort by line number descending (bottom-up)
    fast_ops.sort_by(|a, b| b.line.cmp(&a.line));

    for fop in &fast_ops {
        content = apply_fast_op(&content, fop)?;
        changes.push((fop.line + 1, format!("{:?}", fop.op)));
    }

    // Atomic write
    crate::fast::atomic_write(file, &content)?;
    if let Ok(doc) = crate::document::Document::from_str(file, &content) {
        ctx.modified_doc = Some(doc);
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Applied {} change(s).", changes.len()))
            .map_err(HashlineError::from)?;
    }
    Ok(())
}

fn apply_fast_op(content: &str, fop: &FastOp) -> Result<String, HashlineError> {
    use crate::anchor::try_parse_line_anchor;

    match &fop.op {
        PatchOp::Edit(e) => {
            if let Some((line, hash)) = try_parse_line_anchor(&e.anchor) {
                let (nc, _) = crate::fast::fast_replace_line(content, line, hash, &e.content)?;
                Ok(nc)
            } else {
                Err(HashlineError::PatchFailed { op_index: fop.op_index, reason: "invalid anchor".into() })
            }
        }
        PatchOp::Insert(i) => {
            if let Some((line, _hash)) = try_parse_line_anchor(&i.anchor) {
                crate::fast::fast_insert_line(content, line, &i.content)
            } else {
                Err(HashlineError::PatchFailed { op_index: fop.op_index, reason: "invalid anchor".into() })
            }
        }
        PatchOp::Delete(d) => {
            if let Some((line, hash)) = try_parse_line_anchor(&d.anchor) {
                crate::fast::fast_delete_lines(content, line, line, hash)
            } else {
                Err(HashlineError::PatchFailed { op_index: fop.op_index, reason: "invalid anchor".into() })
            }
        }
    }
}
fn read_patch(path: &str) -> Result<PatchFile, HashlineError> {
    let raw = if path == "-" {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        fs::read_to_string(path)?
    };

    serde_json::from_str(&raw).map_err(HashlineError::from)
}

fn validate_patch_target(patch: &PatchFile, file: &std::path::Path) -> Result<(), HashlineError> {
    if let Some(expected) = &patch.file {
        let actual = file.display().to_string();
        if expected != &actual {
            return Err(HashlineError::PatchFailed {
                op_index: 0,
                reason: format!(
                    "patch file target {expected:?} does not match command target {actual:?}"
                ),
            });
        }
    }

    Ok(())
}

fn build_plan(
    patch: &PatchFile,
    original: &Document,
    index: &crate::document::ShortHashIndex,
) -> Result<Vec<PlannedOp>, HashlineError> {
    let mut plan = Vec::with_capacity(patch.ops.len());
    let mut occupied = vec![None; original.lines.len()];

    for (raw_index, op) in patch.ops.iter().enumerate() {
        let op_index = raw_index + 1;
        let planned = match op {
            PatchOp::Edit(edit) => resolve_edit(op_index, edit, original, index, &mut occupied)?,
            PatchOp::Insert(insert) => resolve_insert(op_index, insert, original, index)?,
            PatchOp::Delete(delete) => {
                resolve_delete(op_index, delete, original, index, &mut occupied)?
            }
        };
        plan.push(planned);
    }

    Ok(plan)
}

fn resolve_edit(
    op_index: usize,
    edit: &EditOp,
    original: &Document,
    index: &crate::document::ShortHashIndex,
    occupied: &mut [Option<Occupancy>],
) -> Result<PlannedOp, HashlineError> {
    validate_single_line_content(&edit.content).map_err(|error| patch_error(op_index, error))?;

    if let Ok(range) = parse_range(&edit.anchor) {
        let (start, end) =
            resolve_range(&range, original, index).map_err(|error| patch_error(op_index, error))?;
        mark_occupied(occupied, start.index..=end.index, Occupancy::Edit, op_index)?;
        let before = original.lines[start.index..=end.index]
            .iter()
            .map(|line| line.content.to_string())
            .collect();
        return Ok(PlannedOp::EditRange {
            op_index,
            range: start.index..=end.index,
            content: edit.content.clone(),
            before,
        });
    }

    let anchor = parse_anchor(&edit.anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved =
        resolve(&anchor, original, index).map_err(|error| patch_error(op_index, error))?;
    mark_occupied(
        occupied,
        resolved.index..=resolved.index,
        Occupancy::Edit,
        op_index,
    )?;
    Ok(PlannedOp::EditSingle {
        op_index,
        line: resolved.index,
        content: edit.content.clone(),
        before: original.lines[resolved.index].content.to_string(),
    })
}

fn resolve_insert(
    op_index: usize,
    insert: &InsertOp,
    original: &Document,
    index: &crate::document::ShortHashIndex,
) -> Result<PlannedOp, HashlineError> {
    validate_single_line_content(&insert.content).map_err(|error| patch_error(op_index, error))?;
    let anchor = parse_anchor(&insert.anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved =
        resolve(&anchor, original, index).map_err(|error| patch_error(op_index, error))?;
    let before = insert.before.unwrap_or(false);
    let boundary = if before {
        resolved.index
    } else {
        resolved.index + 1
    };

    Ok(PlannedOp::Insert {
        boundary,
        content: insert.content.clone(),
        before,
        anchor_line: resolved.line_no,
    })
}

fn resolve_delete(
    op_index: usize,
    delete: &DeleteOp,
    original: &Document,
    index: &crate::document::ShortHashIndex,
    occupied: &mut [Option<Occupancy>],
) -> Result<PlannedOp, HashlineError> {
    let anchor = parse_anchor(&delete.anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved =
        resolve(&anchor, original, index).map_err(|error| patch_error(op_index, error))?;
    mark_occupied(
        occupied,
        resolved.index..=resolved.index,
        Occupancy::Delete,
        op_index,
    )?;
    Ok(PlannedOp::Delete {
        op_index,
        line: resolved.index,
        deleted: original.lines[resolved.index].content.to_string(),
    })
}

fn mark_occupied(
    occupied: &mut [Option<Occupancy>],
    range: RangeInclusive<usize>,
    next: Occupancy,
    op_index: usize,
) -> Result<(), HashlineError> {
    for idx in range {
        if let Some(existing) = occupied[idx] {
            let reason = match existing {
                Occupancy::Edit => format!(
                    "operation overlaps an earlier edit at original line {}",
                    idx + 1
                ),
                Occupancy::Delete => format!(
                    "operation overlaps an earlier delete at original line {}",
                    idx + 1
                ),
            };
            return Err(HashlineError::PatchFailed { op_index, reason });
        }
        occupied[idx] = Some(next);
    }
    Ok(())
}

fn apply_plan(original: &Document, plan: &[PlannedOp]) -> Result<PatchResult, HashlineError> {
    let mut inserts_before: Vec<Vec<String>> = vec![Vec::new(); original.lines.len() + 1];
    let mut replacement_at: Vec<Option<String>> = vec![None; original.lines.len()];
    let mut skip_until: Vec<bool> = vec![false; original.lines.len()];
    let mut deleted = vec![false; original.lines.len()];
    let mut summary = PatchSummary {
        op_count: plan.len(),
        actions: Vec::with_capacity(plan.len()),
        edit_count: 0,
        insert_count: 0,
        delete_count: 0,
    };
    let mut changes = Vec::new();

    for op in plan {
        match op {
            PlannedOp::EditSingle {
                op_index,
                line,
                content,
                before,
            } => {
                let slot =
                    replacement_at
                        .get_mut(*line)
                        .ok_or_else(|| HashlineError::PatchFailed {
                            op_index: *op_index,
                            reason: format!("resolved line {} is out of bounds", line + 1),
                        })?;
                *slot = Some(content.clone());
                changes.push(LineChange {
                    line_no: line + 1,
                    kind: ChangeKind::Modified,
                    before: Some(before.clone()),
                    after: Some(content.clone()),
                });
                summary.edit_count += 1;
                summary.actions.push(format!(
                    "edit line {}: {:?} -> {:?}",
                    line + 1,
                    before,
                    content
                ));
            }
            PlannedOp::EditRange {
                op_index,
                range,
                content,
                before,
            } => {
                let start = *range.start();
                let end = *range.end();
                let slot =
                    replacement_at
                        .get_mut(start)
                        .ok_or_else(|| HashlineError::PatchFailed {
                            op_index: *op_index,
                            reason: format!("resolved start line {} is out of bounds", start + 1),
                        })?;
                *slot = Some(content.clone());
                if let Some(first) = before.first() {
                    changes.push(LineChange {
                        line_no: start + 1,
                        kind: ChangeKind::Modified,
                        before: Some(first.clone()),
                        after: Some(content.clone()),
                    });
                }
                for (offset, removed) in before.iter().enumerate().skip(1) {
                    changes.push(LineChange {
                        line_no: start + offset + 1,
                        kind: ChangeKind::Deleted,
                        before: Some(removed.clone()),
                        after: None,
                    });
                }
                for idx in start + 1..=end {
                    let skip =
                        skip_until
                            .get_mut(idx)
                            .ok_or_else(|| HashlineError::PatchFailed {
                                op_index: *op_index,
                                reason: format!("resolved line {} is out of bounds", idx + 1),
                            })?;
                    *skip = true;
                }
                summary.edit_count += 1;
                summary.actions.push(format!(
                    "edit lines {}-{}: {} line{} replaced",
                    start + 1,
                    end + 1,
                    before.len(),
                    plural_suffix(before.len())
                ));
            }
            PlannedOp::Insert {
                boundary,
                content,
                before,
                anchor_line,
                ..
            } => {
                inserts_before[*boundary].push(content.clone());
                changes.push(LineChange {
                    line_no: boundary + 1,
                    kind: ChangeKind::Inserted,
                    before: None,
                    after: Some(content.clone()),
                });
                summary.insert_count += 1;
                let relation = if *before { "before" } else { "after" };
                summary.actions.push(format!(
                    "insert {:?} {relation} line {}",
                    content, anchor_line
                ));
            }
            PlannedOp::Delete {
                op_index,
                line,
                deleted: content,
            } => {
                let slot = deleted
                    .get_mut(*line)
                    .ok_or_else(|| HashlineError::PatchFailed {
                        op_index: *op_index,
                        reason: format!("resolved line {} is out of bounds", line + 1),
                    })?;
                *slot = true;
                changes.push(LineChange {
                    line_no: line + 1,
                    kind: ChangeKind::Deleted,
                    before: Some(content.clone()),
                    after: None,
                });
                summary.delete_count += 1;
                summary
                    .actions
                    .push(format!("delete line {}: {:?}", line + 1, content));
            }
        }
    }

    let mut new_contents = Vec::new();
    for boundary in 0..=original.lines.len() {
        new_contents.extend(inserts_before[boundary].iter().cloned());
        if boundary == original.lines.len() {
            continue;
        }
        if skip_until[boundary] || deleted[boundary] {
            continue;
        }
        if let Some(replacement) = &replacement_at[boundary] {
            new_contents.push(replacement.clone());
        } else {
            new_contents.push(original.lines[boundary].content.to_string());
        }
    }

    let mut document = original.clone();
    document.lines = build_lines(&new_contents);
    if document.lines.is_empty() {
        document.trailing_newline = false;
    }

    Ok(PatchResult {
        document,
        summary,
        changes,
    })
}

fn build_lines(contents: &[String]) -> Vec<LineRecord> {
    contents
        .iter()
        .map(|content| {
            let full_hash = hash::full_hash(content);
            LineRecord {
                content: Box::from(content.as_str()),
                short_hash: hash::short_from_full(full_hash),
            }
        })
        .collect()
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &std::path::Path,
    summary: &PatchSummary,
    changes: &[LineChange],
) -> Result<(), HashlineError> {
    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            // PR-D: emit a compact mutation receipt instead of dumping the
            // entire proposed document.
            let dry_run_summary = summary
                .success_message()
                .replacen("Applied", "Would apply", 1);
            let receipt =
                receipt::build_dry_run_receipt("patch", file, dry_run_summary, changes.to_vec());
            receipt::write_dry_run_receipt(ctx, &receipt)
        }
        OutputMode::Pretty => {
            let dry_run_message = summary
                .success_message()
                .replacen("Applied", "Would apply", 1);
            output::write_success_line(ctx, &dry_run_message)?;
            for action in &summary.actions {
                output::write_success_line(ctx, &format!("  - {action}"))?;
            }
            output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)
        }
    }
}

fn patch_error(op_index: usize, error: HashlineError) -> HashlineError {
    HashlineError::PatchFailed {
        op_index,
        reason: error.to_string(),
    }
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
