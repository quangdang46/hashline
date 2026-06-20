//! Recover from a stale section snapshot tag by replaying the would-be edit
//! against a cached pre-edit snapshot of the file and 3-way-merging the
//! result onto the current on-disk content.
//!
//! The patcher consults this module when a section tag resolves to a
//! snapshot that no longer matches the live file content. Recovery is
//! stateless apart from the [`SnapshotStore`] it queries; the snapshot
//! store is the seam that lets you plug in your own caching strategy.
//!
//! Two strategies are tried in order:
//!
//! 1. Apply the edits to the snapshot text, then 3-way-merge the
//!    resulting patch onto the live content (handles external writes).
//! 2. (Session chain) If the snapshot was not the head, replay the edits
//!    onto the live content directly when line counts match AND every
//!    edit's anchor line content is unchanged between snapshot and
//!    current — a prior in-session edit advanced the file and the
//!    model's anchors still name the same logical rows.

use crate::messages::{
    RECOVERY_EXTERNAL_WARNING, RECOVERY_SESSION_CHAIN_WARNING, RECOVERY_SESSION_REPLAY_WARNING,
};
use crate::merge::merge_texts;
use crate::snapshot_store::SnapshotStore;
use crate::types::{Anchor, ApplyResult, Cursor, Edit};

// ---------------------------------------------------------------------------
// Recovery types
// ---------------------------------------------------------------------------

/// Arguments for a recovery attempt.
pub struct RecoveryArgs {
    pub path: String,
    pub current_text: String,
    pub file_hash: String,
    pub edits: Vec<Edit>,
}

/// The result of a successful recovery attempt.
pub struct RecoveryResult {
    /// Post-recovery text.
    pub text: String,
    /// First changed line (1-indexed) relative to the live `current_text`,
    /// or `None` if no net change was detected.
    pub first_changed_line: Option<usize>,
    /// Warnings collected during recovery, including the user-facing
    /// recovery-mode banner.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Recovery driver
// ---------------------------------------------------------------------------

/// Stateless recovery driver over a [`SnapshotStore`].
///
/// Construct once and call [`Recovery::try_recover`] per stale-tag
/// incident. The default implementation tries two strategies in order
/// (see [module-level docs](self) for details).
pub struct Recovery<'a> {
    store: &'a dyn SnapshotStore,
}

impl<'a> Recovery<'a> {
    pub fn new(store: &'a dyn SnapshotStore) -> Self {
        Self { store }
    }

    /// Attempt to recover from a stale snapshot tag.
    ///
    /// `apply_edits` applies a set of edits to a text body, returning
    /// `Ok(ApplyResult)` on success or `Err(String)` on failure.
    /// It is accepted as a closure so the caller can inject the real
    /// implementation (or a test double) — the `apply::apply_edits`
    /// module is a stub at this stage.
    ///
    /// Returns `None` when no path forward is found — the caller should
    /// then surface a mismatch error.
    pub fn try_recover<F>(
        &self,
        args: &RecoveryArgs,
        apply_edits: F,
    ) -> Option<RecoveryResult>
    where
        F: Fn(&str, &[Edit]) -> Result<ApplyResult, String>,
    {
        let snapshot = self.store.by_hash(&args.path, &args.file_hash)?;
        let head = self.store.head(&args.path);
        let is_head = head
            .as_ref()
            .is_some_and(|h| h.hash == args.file_hash);

        let recovery_warning = if is_head {
            RECOVERY_EXTERNAL_WARNING
        } else {
            RECOVERY_SESSION_CHAIN_WARNING
        };

        // Strategy 1: apply edits to snapshot, then 3-way-merge the
        // resulting diff onto the live content (handles external writes).
        if let Some(result) = apply_edits_to_snapshot(
            &snapshot.text,
            &args.current_text,
            &args.edits,
            recovery_warning,
            &apply_edits,
        ) {
            return Some(result);
        }

        // Strategy 2 (session-chain fallback): if the snapshot wasn't the
        // head, try replaying the edits directly onto current.  Guarded by
        // line-count equality AND anchor-content alignment — see
        // `replay_session_chain_on_current` for why even both guards
        // together don't fully prove correctness.
        if !is_head {
            return replay_session_chain_on_current(
                &snapshot.text,
                &args.current_text,
                &args.edits,
                &apply_edits,
            );
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Strategy 1: 3-way merge
// ---------------------------------------------------------------------------

/// Apply the edits to the snapshot text, then 3-way-merge the result onto
/// the live current text.
///
/// Returns `None` when:
/// - the edits could not be applied to the snapshot,
/// - the edits produced no change to the snapshot,
/// - the 3-way merge produced conflicts, or
/// - the merged result is identical to `current_text`.
fn apply_edits_to_snapshot<F>(
    previous_text: &str,
    current_text: &str,
    edits: &[Edit],
    recovery_warning: &str,
    apply_fn: &F,
) -> Option<RecoveryResult>
where
    F: Fn(&str, &[Edit]) -> Result<ApplyResult, String>,
{
    // Apply the edits against the snapshot (the old version).
    let applied = apply_fn(previous_text, edits).ok()?;
    if applied.text == previous_text {
        return None;
    }

    // 3-way merge: base=snapshot, target=applied, current=live.
    let merged = merge_texts(previous_text, &applied.text, current_text);
    if merged.conflict_count > 0 || merged.result == current_text {
        return None;
    }

    let first_changed_line =
        find_first_changed_line(current_text, &merged.result).or(applied.first_changed_line);

    let mut warnings: Vec<String> = if first_changed_line.is_some() {
        vec![recovery_warning.to_string()]
    } else {
        Vec::new()
    };
    warnings.extend(applied.warnings);

    Some(RecoveryResult {
        text: merged.result,
        first_changed_line,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Strategy 2: session-chain replay
// ---------------------------------------------------------------------------

/// Replay the edits directly onto `current_text`, gated by two guards
/// that narrow the corruption window:
///
/// 1. **Equal line counts** — every line number in `edits` still resolves
///    to *some* logical row (no net shift across the prior session chain).
/// 2. **Anchor-content alignment** — the row at each anchor's line index
///    has identical content in `previous_text` and `current_text`. Catches
///    the common case of a prior edit rewriting the targeted line.
///
/// Neither guard alone is sufficient, and even together they don't fully
/// prove correctness — replay is the less-certain recovery mode and emits
/// [`RECOVERY_SESSION_REPLAY_WARNING`] so the caller can verify the diff.
fn replay_session_chain_on_current<F>(
    previous_text: &str,
    current_text: &str,
    edits: &[Edit],
    apply_fn: &F,
) -> Option<RecoveryResult>
where
    F: Fn(&str, &[Edit]) -> Result<ApplyResult, String>,
{
    // Guard 1: equal line counts.
    if split_lines(previous_text).len() != split_lines(current_text).len() {
        return None;
    }

    // Guard 2: anchor-line content unchanged between snapshot and current.
    if !verify_anchor_content(previous_text, current_text, edits) {
        return None;
    }

    let applied = apply_fn(current_text, edits).ok()?;
    if applied.text == current_text {
        return None;
    }

    Some(RecoveryResult {
        first_changed_line: applied.first_changed_line,
        text: applied.text,
        warnings: std::iter::once(RECOVERY_SESSION_REPLAY_WARNING.to_string())
            .chain(applied.warnings.into_iter())
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// First 1-indexed line at which `a` and `b` diverge, or `None` if equal.
fn find_first_changed_line(a: &str, b: &str) -> Option<usize> {
    if a == b {
        return None;
    }

    let a_lines: Vec<&str> = a.split('\n').collect();
    let b_lines: Vec<&str> = b.split('\n').collect();
    let max = a_lines.len().max(b_lines.len());

    for i in 0..max {
        if a_lines.get(i) != b_lines.get(i) {
            return Some(i + 1);
        }
    }

    None
}

/// Split text into lines, mirroring the `split("\n")` used in the
/// TypeScript original and in `merge.rs`.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// Collect every line number that the given edits reference as an anchor.
fn collect_anchor_lines(edits: &[Edit]) -> Vec<usize> {
    let mut lines = Vec::new();
    for edit in edits {
        for anchor in get_edit_anchors(edit) {
            lines.push(anchor.line);
        }
    }
    lines
}

/// Extract all anchor points from a single edit.
///
/// Corresponds to the TypeScript `getEditAnchors`:
/// - `Delete` -> its anchor
/// - `Block`  -> its anchor
/// - `Insert` with `BeforeAnchor` / `AfterAnchor` -> the cursor's anchor
/// - `Insert` with `Bof` / `Eof` -> empty (no anchor to verify)
fn get_edit_anchors(edit: &Edit) -> Vec<Anchor> {
    match edit {
        Edit::Delete { anchor, .. } => vec![*anchor],
        Edit::Block { anchor, .. } => vec![*anchor],
        Edit::Insert { cursor, .. } => match cursor {
            Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a) => vec![*a],
            _ => vec![],
        },
    }
}

/// Returns `true` when every anchor line in `edits` has identical content
/// in `previous_text` and `current_text`.
///
/// The session-chain replay fast-path requires this: if the prior
/// in-session edit rewrote the line the model is now re-targeting with a
/// stale hash, replaying onto current would silently overwrite the new
/// content with whatever the model authored against the old content —
/// a corruption window, not a recovery.
fn verify_anchor_content(previous_text: &str, current_text: &str, edits: &[Edit]) -> bool {
    let lines = collect_anchor_lines(edits);
    if lines.is_empty() {
        return true;
    }

    let prev: Vec<&str> = previous_text.split('\n').collect();
    let curr: Vec<&str> = current_text.split('\n').collect();

    for &line in &lines {
        // Line numbers are 1-indexed; convert to 0-indexed.
        if line == 0 {
            return false;
        }
        let idx = line - 1;
        if idx >= prev.len() || idx >= curr.len() {
            return false;
        }
        if prev[idx] != curr[idx] {
            return false;
        }
    }

    true
}
