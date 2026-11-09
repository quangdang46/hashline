//! Error and warning message constants for the hashline parser, applier,
//! patcher, and recovery subsystems.

// === Envelope markers ===

pub const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub const END_PATCH_MARKER: &str = "*** End Patch";
pub const ABORT_MARKER: &str = "*** Abort";

// === Parser warnings ===

pub const BARE_BODY_AUTO_PIPED_WARNING: &str =
    "bare body auto-piped (no `+` prefix) — model should prefix body rows with `+` to be explicit";
pub const MINUS_ROW_REJECTED: &str = "`-` rows accepted as content but should use `+-` prefix (e.g. `+- list item`) for explicit neutral payload; bare `-` is reserved for future diff syntax";

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
