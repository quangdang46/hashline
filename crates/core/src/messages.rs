//! Error and warning message constants for the hashline parser, applier,
//! patcher, and recovery subsystems.

// === Envelope markers ===

pub const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub const END_PATCH_MARKER: &str = "*** End Patch";
pub const ABORT_MARKER: &str = "*** Abort";

// === Parser warnings ===

pub const BARE_BODY_AUTO_PIPED_WARNING: &str =
    "bare body auto-piped (no `+` prefix) — model should prefix body rows with `+` to be explicit";
pub const MINUS_ROW_REJECTED: &str =
    "`-` rows are not valid in hashline; only `+TEXT` payload lines are accepted";
/// Warning when a raw line looks like it's trying to be a hunk operation
/// but uses an unknown keyword. Example: `FOO 1:` or `BAR.BAZ 5:`.
pub const UNKNOWN_OP_ROW: &str = "unknown operation";

pub const EMPTY_BLOCK: &str =
    "block replacement has no body; provide replacement lines after the header";
pub const EMPTY_INSERT: &str = "insert has no body; provide content after the header";
pub const DELETE_TAKES_NO_BODY: &str =
    "delete takes no body; remove payload lines after a delete header";
pub const DELETE_BLOCK_TAKES_NO_BODY: &str =
    "delete block takes no body; remove payload lines after a delete block header";

// === Apply warnings ===

pub const UNRESOLVED_BLOCK_INTERNAL: &str =
    "internal: block edit reached apply_edits without resolution";

// === Apply-patch path noise constants ===

/// Prefixes LLMs commonly prepend to file paths when generating apply-patch style output.
pub const APPLY_PATCH_UPDATE_PREFIX: &str = "Update File:";
pub const APPLY_PATCH_ADD_PREFIX: &str = "Add File:";
pub const APPLY_PATCH_DELETE_PREFIX: &str = "Delete File:";
pub const APPLY_PATCH_MOVE_PREFIX: &str = "Move to:";

// === Recovery warnings ===

/// File was modified by an external process between the snapshot tag was minted
/// and the edit was applied. The edit was recovered via 3-way merge.
pub const RECOVERY_EXTERNAL_WARNING: &str =
    "file changed after snapshot; applied edit via 3-way merge";

/// A subsequent in-session edit advanced the file past the version the section
/// tag names. The edit was recovered by replaying against the session chain.
pub const RECOVERY_SESSION_CHAIN_WARNING: &str =
    "edit targeted a prior in-session version; recovered via session-chain merge";

/// Replayed edits directly onto current content because line counts matched
/// and anchor-line content was unchanged. This is the less-certain recovery
/// mode and the result should be manually verified.
pub const RECOVERY_SESSION_REPLAY_WARNING: &str = "edit replayed onto current content (anchor lines unchanged, line counts matched) — verify the result";

/// Only head/tail inserts are safe to apply when the snapshot tag is stale.
pub const HEADTAIL_DRIFT_WARNING: &str =
    "file has changed since the snapshot was taken; head/tail-only inserts applied to live content";

// === Error message builders ===

/// Message when a patch section has no snapshot tag and contains anchored edits.
pub fn missing_snapshot_tag_message(section_path: &str) -> String {
    format!(
        "File {section_path} has no snapshot tag. \
         Use `read` to get the current snapshot hash, then include `[path#HASH]` \
         in your patch header."
    )
}

/// Message when an anchor references a line the model has not seen.
pub fn unseen_lines_message(path: &str, unseen: &[usize], expected: &str) -> String {
    let lines = unseen
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "File {path}: snapshot #{expected} does not include line(s) {lines}. \
         Re-read the file and retry with lines that were actually displayed."
    )
}

/// Format a block-unresolved error message.
pub fn block_unresolved_message(line: usize, op: &str) -> String {
    format!("line {line}: cannot resolve block anchor for `{op}` — no block resolver available")
}

/// Landing shift warning for `after_anchor` inserts (outward).
pub fn after_insert_landing_shift_warning(anchor_line: usize, landing_line: usize) -> String {
    format!(
        "insert body indentation suggests it belongs after line {landing_line}, \
         not after line {anchor_line}; shifted landing accordingly"
    )
}

/// Landing shift warning for block inserts (inward).
pub fn block_insert_landing_shift_warning(
    block_start: usize,
    _closer_line: usize,
    landing_line: usize,
) -> String {
    format!(
        "block insert body indentation suggests it belongs inside the block (line {landing_line}), \
         not after the closing line; shifted landing inward from block start at line {block_start}"
    )
}
