use std::fs;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::anchor::{parse_anchor, parse_range, resolve, resolve_range};
use crate::cli::MergePatchesCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: MergePatchesCmd,
) -> Result<(), LinehashError> {
    let patch_a = read_patch(&cmd.patch_a)?;
    let patch_b = read_patch(&cmd.patch_b)?;
    validate_patch_target(&patch_a, &cmd.base)?;
    validate_patch_target(&patch_b, &cmd.base)?;

    let base = Document::load(&cmd.base)?;
    let index = base.build_index();
    let mut resolved_a = resolve_patch_ops(&patch_a, &base, &index, "A")?;
    let mut resolved_b = resolve_patch_ops(&patch_b, &base, &index, "B")?;
    let conflicts = collect_conflicts(&mut resolved_a, &mut resolved_b);
    let merged_patch = PatchFile {
        file: Some(cmd.base.display().to_string()),
        ops: resolved_a
            .iter()
            .filter(|op| !op.conflicted)
            .chain(resolved_b.iter().filter(|op| !op.conflicted))
            .map(|op| op.op.clone())
            .collect(),
    };

    match ctx.output_mode() {
        OutputMode::Json => output::write_json_success(
            ctx,
            &MergeOutput {
                merged_patch,
                conflicts,
            },
        )
        .map_err(LinehashError::from),
        OutputMode::Pretty | OutputMode::Ndjson => write_pretty(ctx, &merged_patch, &conflicts),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PatchFile {
    file: Option<String>,
    ops: Vec<PatchOp>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum PatchOp {
    Edit {
        anchor: String,
        content: String,
    },
    Insert {
        anchor: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<bool>,
    },
    Delete {
        anchor: String,
    },
}

#[derive(Clone, Debug, Serialize)]
struct MergeOutput {
    merged_patch: PatchFile,
    conflicts: Vec<ConflictRecord>,
}

#[derive(Clone, Debug, Serialize)]
struct ConflictRecord {
    patch_a_op: usize,
    patch_b_op: usize,
    target: String,
    patch_a: PatchOp,
    patch_b: PatchOp,
}

#[derive(Clone, Debug)]
struct ResolvedOp {
    op_index: usize,
    target: TargetKind,
    target_label: String,
    op: PatchOp,
    conflicted: bool,
}

#[derive(Clone, Debug)]
enum TargetKind {
    Lines(RangeInclusive<usize>),
    Boundary(usize),
}

fn read_patch(path: &Path) -> Result<PatchFile, LinehashError> {
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(LinehashError::from)
}

fn validate_patch_target(patch: &PatchFile, base: &Path) -> Result<(), LinehashError> {
    if let Some(expected) = &patch.file {
        let actual = base.display().to_string();
        if expected != &actual {
            return Err(LinehashError::PatchFailed {
                op_index: 0,
                reason: format!(
                    "patch file target {expected:?} does not match command target {actual:?}"
                ),
            });
        }
    }
    Ok(())
}

fn resolve_patch_ops(
    patch: &PatchFile,
    base: &Document,
    index: &crate::document::ShortHashIndex,
    _source: &'static str,
) -> Result<Vec<ResolvedOp>, LinehashError> {
    let mut resolved = Vec::with_capacity(patch.ops.len());
    for (raw_index, op) in patch.ops.iter().enumerate() {
        let op_index = raw_index + 1;
        let (target, target_label) = match op {
            PatchOp::Edit { anchor, .. } => resolve_edit_target(anchor, base, index, op_index)?,
            PatchOp::Insert { anchor, before, .. } => {
                resolve_insert_target(anchor, before.unwrap_or(false), base, index, op_index)?
            }
            PatchOp::Delete { anchor } => resolve_delete_target(anchor, base, index, op_index)?,
        };
        resolved.push(ResolvedOp {
            op_index,
            target,
            target_label,
            op: op.clone(),
            conflicted: false,
        });
    }
    Ok(resolved)
}

fn resolve_edit_target(
    anchor: &str,
    base: &Document,
    index: &crate::document::ShortHashIndex,
    op_index: usize,
) -> Result<(TargetKind, String), LinehashError> {
    if let Ok(range) = parse_range(anchor) {
        let (start, end) =
            resolve_range(&range, base, index).map_err(|error| patch_error(op_index, error))?;
        return Ok((
            TargetKind::Lines(start.index..=end.index),
            anchor.to_string(),
        ));
    }
    let parsed = parse_anchor(anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved = resolve(&parsed, base, index).map_err(|error| patch_error(op_index, error))?;
    Ok((
        TargetKind::Lines(resolved.index..=resolved.index),
        anchor.to_string(),
    ))
}

fn resolve_insert_target(
    anchor: &str,
    before: bool,
    base: &Document,
    index: &crate::document::ShortHashIndex,
    op_index: usize,
) -> Result<(TargetKind, String), LinehashError> {
    let parsed = parse_anchor(anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved = resolve(&parsed, base, index).map_err(|error| patch_error(op_index, error))?;
    let boundary = if before {
        resolved.index
    } else {
        resolved.index + 1
    };
    let relation = if before { "before" } else { "after" };
    Ok((
        TargetKind::Boundary(boundary),
        format!("{relation} {anchor}"),
    ))
}

fn resolve_delete_target(
    anchor: &str,
    base: &Document,
    index: &crate::document::ShortHashIndex,
    op_index: usize,
) -> Result<(TargetKind, String), LinehashError> {
    let parsed = parse_anchor(anchor).map_err(|error| patch_error(op_index, error))?;
    let resolved = resolve(&parsed, base, index).map_err(|error| patch_error(op_index, error))?;
    Ok((
        TargetKind::Lines(resolved.index..=resolved.index),
        anchor.to_string(),
    ))
}

fn collect_conflicts(a: &mut [ResolvedOp], b: &mut [ResolvedOp]) -> Vec<ConflictRecord> {
    let mut conflicts = Vec::new();

    for a_op in a.iter_mut() {
        for b_op in b.iter_mut() {
            if overlaps(&a_op.target, &b_op.target) {
                a_op.conflicted = true;
                b_op.conflicted = true;
                conflicts.push(ConflictRecord {
                    patch_a_op: a_op.op_index,
                    patch_b_op: b_op.op_index,
                    target: a_op.target_label.clone(),
                    patch_a: a_op.op.clone(),
                    patch_b: b_op.op.clone(),
                });
            }
        }
    }

    conflicts
}

fn overlaps(left: &TargetKind, right: &TargetKind) -> bool {
    match (left, right) {
        (TargetKind::Lines(a), TargetKind::Lines(b)) => {
            a.start() <= b.end() && b.start() <= a.end()
        }
        (TargetKind::Boundary(a), TargetKind::Boundary(b)) => a == b,
        (TargetKind::Boundary(boundary), TargetKind::Lines(lines))
        | (TargetKind::Lines(lines), TargetKind::Boundary(boundary)) => {
            *lines.start() <= *boundary && *boundary <= (*lines.end() + 1)
        }
    }
}

fn write_pretty<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    merged_patch: &PatchFile,
    conflicts: &[ConflictRecord],
) -> Result<(), LinehashError> {
    // Pretty (text) mode embeds the merged patch JSON as a human-readable block,
    // so the embedded JSON stays pretty-printed regardless of the global style.
    if conflicts.is_empty() {
        serde_json::to_writer_pretty(ctx.stdout(), merged_patch)?;
        writeln!(ctx.stdout()).map_err(LinehashError::from)
    } else {
        for conflict in conflicts {
            output::write_success_line(
                ctx,
                &format!(
                    "CONFLICT: op {} in patch A and op {} in patch B both target {}",
                    conflict.patch_a_op, conflict.patch_b_op, conflict.target
                ),
            )?;
        }
        output::write_success_line(ctx, "Merged non-conflicting ops:")?;
        serde_json::to_writer_pretty(ctx.stdout(), merged_patch)?;
        writeln!(ctx.stdout()).map_err(LinehashError::from)
    }
}

fn patch_error(op_index: usize, error: LinehashError) -> LinehashError {
    LinehashError::PatchFailed {
        op_index,
        reason: error.to_string(),
    }
}
