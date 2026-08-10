//! High-level [`Editor`] API for embedding hashline in other tools.
//!
//! # Zero dot-folder design
//!
//! The [`Editor`] struct is hashline's library-first entry point. It accepts
//! dependencies via trait objects — no filesystem state, no config files,
//! no environment variables. Consumers bring their own [`SnapshotStore`] and
//! optionally their own [`BlockResolver`].
//!
//! ```rust,ignore
//! use hashline::{
//!     Editor, HashlineConfig,
//!     snapshot_store::InMemorySnapshotStore,
//! };
//!
//! let mut editor = Editor::with_store(
//!     InMemorySnapshotStore::new(),
//!     HashlineConfig::default(),
//! );
//! let result = editor.patch("src/main.rs", "SWAP 3:a7:\n+replaced")?;
//! ```
//!
//! No `.hashline/` directory is created. No config file is read.
//! Every concern is injected at construction time.
//!
//! # Thread safety
//!
//! [`Editor`] is not `Sync` by default because `SnapshotStore` methods
//! take `&mut self`. Wrap in `Mutex` for shared access.

use std::path::{Path, PathBuf};

use crate::block::resolve_block_edits;
use crate::config::HashlineConfig;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::hash;
use crate::normalize::{LineEnding, detect_line_ending, restore_line_endings};
use crate::parser::parse_patch;
use crate::snapshot_store::SnapshotStore;
use crate::types::{BlockResolver as BlockResolverTrait, Edit};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of an [`Editor::read`] call.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ReadResult {
    /// Absolute or relative path of the file.
    pub path: PathBuf,
    /// 4-hex content hash.
    pub hash: String,
    /// Lines with their hashes (trailing empty line omitted).
    pub lines: Vec<LineWithHash>,
}

/// A single line with its short hash.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LineWithHash {
    /// 1-based line number.
    pub n: usize,
    /// 2-char short xxh32 hash.
    pub hash: String,
    /// Line content (without newline).
    pub content: String,
}

/// Result of an [`Editor::patch`] call.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PatchResult {
    /// Number of edits successfully applied.
    pub applied_edits: usize,
    /// 4-hex content hash of the resulting file.
    pub hash: String,
    /// Diagnostics emitted by the parser/applier.
    pub warnings: Vec<String>,
    /// Full resulting text after patch.
    pub text: String,
    /// Whether this was a dry-run (no file was written).
    pub dry_run: bool,
}

/// Result of an [`Editor::write`] call.
#[derive(Clone, Debug, serde::Serialize)]
pub struct WriteResult {
    /// Path of the written file.
    pub path: PathBuf,
    /// 4-hex content hash of the written file.
    pub hash: String,
    /// Lines with their hashes.
    pub lines: Vec<LineWithHash>,
}

/// Result of an [`Editor::find_block`] call.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FindBlockResult {
    /// Display name of the detected programming language (if known).
    pub language: Option<String>,
    /// Total lines in the file.
    pub line_count: usize,
    /// Lines belonging to the syntactic block (inclusive, 0-indexed).
    pub block_lines: Vec<LineWithHash>,
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

/// High-level API for hashline editing operations.
///
/// Wraps a [`SnapshotStore`] (for caching file snapshots) and an optional
/// [`BlockResolver`](BlockResolverTrait) (for syntactic block operations).
/// Every method returns a typed result — no I/O writers, no CLI context.
pub struct Editor {
    snapshot_store: Box<dyn SnapshotStore>,
    block_resolver: Option<Box<dyn BlockResolverTrait>>,
    config: HashlineConfig,
    noop_guard: crate::noop_guard::NoopGuard,
}

impl Editor {
    /// Create an `Editor` with a custom snapshot store and default config.
    pub fn with_store(store: impl SnapshotStore + 'static) -> Self {
        Self {
            snapshot_store: Box::new(store),
            block_resolver: None,
            config: HashlineConfig::default(),
            noop_guard: crate::noop_guard::NoopGuard::new(
                HashlineConfig::default().noop_guard_limit,
            ),
        }
    }

    /// Create an `Editor` with a custom store and config.
    pub fn new(store: impl SnapshotStore + 'static, config: HashlineConfig) -> Self {
        Self {
            snapshot_store: Box::new(store),
            block_resolver: None,
            config: config.clone(),
            noop_guard: crate::noop_guard::NoopGuard::new(config.noop_guard_limit),
        }
    }

    /// Convenience: create an `Editor` with no snapshot caching
    /// (equivalent to `NoopSnapshotStore`).
    pub fn without_snapshots() -> Self {
        let config = HashlineConfig {
            enable_snapshots: false,
            ..HashlineConfig::default()
        };
        Self {
            snapshot_store: Box::new(crate::snapshot_store::NoopSnapshotStore),
            block_resolver: None,
            noop_guard: crate::noop_guard::NoopGuard::new(config.noop_guard_limit),
            config,
        }
    }

    /// Set a custom block resolver.
    pub fn with_block_resolver(mut self, resolver: impl BlockResolverTrait + 'static) -> Self {
        self.block_resolver = Some(Box::new(resolver));
        self
    }

    /// Set the built-in heuristic block resolver.
    pub fn with_builtin_resolver(mut self) -> Self {
        self.block_resolver = Some(Box::new(crate::builtin_resolver::BuiltinBlockResolver));
        self
    }

    /// Set custom config (builder style).
    pub fn with_config(mut self, config: HashlineConfig) -> Self {
        self.config = config;
        self
    }

    /// Return a reference to the snapshot store (for inspection).
    pub fn snapshot_store(&self) -> &dyn SnapshotStore {
        &*self.snapshot_store
    }

    /// Return a mutable reference to the snapshot store.
    pub fn snapshot_store_mut(&mut self) -> &mut dyn SnapshotStore {
        &mut *self.snapshot_store
    }

    // ------------------------------------------------------------------
    // Read
    // ------------------------------------------------------------------

    /// Read a file and return its content with per-line hashes.
    ///
    /// Records the file state in the snapshot store for subsequent stale-anchor
    /// detection during patching.
    pub fn read(&mut self, path: &Path) -> Result<ReadResult, HashlineError> {
        let fc = FileContent::load(path)?;
        let entries = fc.lines_with_hashes();
        let raw_lines = fc.lines();

        // Record snapshot (if enabled)
        if self.config.enable_snapshots {
            let seen: Vec<usize> = (1..=raw_lines.len()).collect();
            self.snapshot_store
                .record(&fc.path.to_string_lossy(), &fc.normalized, Some(&seen));
        }

        // Build result lines (skip trailing empty from final newline)
        let lines: Vec<LineWithHash> = entries
            .iter()
            .enumerate()
            .filter(|(i, entry)| {
                !(entry.content.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline)
            })
            .map(|(i, entry)| LineWithHash {
                n: i + 1,
                hash: hash::format_short_hash(entry.short_hash),
                content: entry.content.clone(),
            })
            .collect();

        Ok(ReadResult {
            path: fc.path,
            hash: fc.hash,
            lines,
        })
    }

    // ------------------------------------------------------------------
    // Patch
    // ------------------------------------------------------------------

    /// Apply a hashline patch to a file.
    ///
    /// 1. Reads the current file content.
    /// 2. Parses the patch string into edits.
    /// 3. Resolves any `SWAP.BLK` / `DEL.BLK` / `INS.BLK.*` operations using
    ///    the configured block resolver.
    /// 4. Applies edits to the in-memory text.
    /// 5. Writes the result back to disk.
    /// 6. Records the new file state in the snapshot store.
    pub fn patch(&mut self, path: &Path, patch_str: &str) -> Result<PatchResult, HashlineError> {
        self.patch_inner(path, patch_str, false)
    }

    /// Like [`Editor::patch`] but does not write to disk.
    /// Returns the diff and final text for preview.
    pub fn dry_run(&mut self, path: &Path, patch_str: &str) -> Result<PatchResult, HashlineError> {
        self.patch_inner(path, patch_str, true)
    }

    fn patch_inner(
        &mut self,
        path: &Path,
        patch_str: &str,
        dry_run: bool,
    ) -> Result<PatchResult, HashlineError> {
        let fc = FileContent::load(path)?;
        let text = &fc.normalized;
        let (edits, warnings, _file_op, aborted) = parse_patch(patch_str);

        // Handle abort / empty patch
        if edits.is_empty() {
            if aborted {
                return Ok(PatchResult {
                    applied_edits: 0,
                    hash: fc.hash.clone(),
                    warnings,
                    text: text.clone(),
                    dry_run,
                });
            }
            return if warnings.is_empty() {
                Err(HashlineError::EmptyPatch)
            } else {
                Err(HashlineError::EmptyPatchWithReason {
                    reason: warnings[0].clone().into_boxed_str(),
                })
            };
        }

        // Resolve block edits (plugins can provide their own resolver)
        let resolved = resolve_block_edits(
            &edits,
            text,
            &path.to_string_lossy(),
            self.block_resolver.as_deref(),
        )
        .map_err(|msg| HashlineError::BlockUnresolved {
            line: 0,
            message: msg,
        })?;

        // Apply edits to in-memory lines. On a stale anchor, attempt
        // snapshot-based recovery (Phase 3): replay the edits against the
        // cached snapshot whose hashes match, then 3-way-merge onto the live
        // content. Fail-closed — if recovery is unavailable or ambiguous, the
        // original StaleAnchor error is surfaced.
        let mut lines: Vec<String> = split_normalized(text);
        let entries = fc.lines_with_hashes();
        let had_trailing_newline = fc.trailing_newline;

        let apply_result = crate::commands::patch::apply_edits(&mut lines, &entries, path, &resolved);
        if let Err(HashlineError::StaleAnchor { .. }) = apply_result {
            if self.config.enable_snapshots {
                let recovery = crate::recovery::Recovery::new(&*self.snapshot_store);
                // The anchor came from the most-recently-read snapshot of this
                // path, not the current (drifted) file. `try_recover` looks the
                // snapshot up by its tag; use the head snapshot's hash.
                let snapshot_hash = self
                    .snapshot_store
                    .head(&path.to_string_lossy())
                    .map(|s| s.hash)
                    .unwrap_or_else(|| fc.hash.clone());
                let args = crate::recovery::RecoveryArgs {
                    path: path.to_string_lossy().into_owned(),
                    current_text: text.clone(),
                    file_hash: snapshot_hash,
                    edits: resolved.clone(),
                };
                let apply_fn = |snapshot_text: &str, edits: &[Edit]| {
                    crate::commands::patch::apply_edits_pure(snapshot_text, edits, path)
                        .map_err(|e| e.to_string())
                };
                if let Some(recovered) = recovery.try_recover(&args, apply_fn) {
                    for w in &recovered.warnings {
                        eprintln!("warning: {w}");
                    }
                    // Rejoin the recovered text (LF-normalized already).
                    lines = split_normalized(&recovered.text);
                    let _ = had_trailing_newline;
                } else {
                    // Recovery failed — surface the original stale error.
                    apply_result?;
                }
            } else {
                apply_result?;
            }
        } else {
            apply_result?;
        }

        // Rejoin
        let result = if had_trailing_newline && !lines.is_empty() {
            lines.join("\n") + "\n"
        } else if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n")
        };

        let line_ending = detect_line_ending(&fc.raw);
        let final_text = if line_ending == LineEnding::Crlf {
            restore_line_endings(&result, line_ending)
        } else {
            result
        };

        let new_hash = hash::compute_file_hash(&final_text);

        // No-op loop guard: a patch that produces no net content change on the
        // same path with the same text, repeated `noop_guard_limit` times, is a
        // loop — surface a hard error so the agent re-reads and re-anchors
        // instead of spinning.
        let was_noop = final_text == *text;
        if !dry_run && self.config.noop_guard_enabled {
            let fp = crate::noop_guard::fingerprint(patch_str);
            let path_str = path.to_string_lossy();
            if let Err(streak) = self.noop_guard.record(&path_str, fp, was_noop) {
                return Err(HashlineError::NoopLoop {
                    path: path.to_string_lossy().into_owned().into_boxed_str(),
                    attempts: streak,
                });
            }
            if !was_noop {
                // A real change resets the streak; nothing more to do.
            }
        }

        if !dry_run {
            crate::commands::common::fast_write(path, final_text.as_bytes())?;
        }

        // Record new snapshot (if enabled)
        if self.config.enable_snapshots && !dry_run {
            self.snapshot_store
                .record(&path.to_string_lossy(), &final_text, None);
        }

        // Invalidate old snapshot entry
        if !dry_run {
            self.snapshot_store.invalidate(&path.to_string_lossy());
        }

        Ok(PatchResult {
            applied_edits: resolved.len(),
            hash: new_hash,
            warnings,
            text: final_text,
            dry_run,
        })
    }
    // ------------------------------------------------------------------
    // In-memory apply (no disk I/O)
    // ------------------------------------------------------------------

    /// Apply a hashline patch to an in-memory text buffer.
    ///
    /// Unlike [`Editor::patch`], this method does **not**:
    /// - Read or write to disk
    /// - Record or invalidate snapshots
    /// - Detect stale anchors (no prior snapshot context)
    ///
    /// It parses the patch, resolves block edits, applies them to the text,
    /// and returns the resulting text with metadata. Perfect for consumers
    /// like next-code that manage their own file I/O, snapshot stores, and
    /// bus events.
    ///
    /// `path` is a display name passed through to block resolution (language
    /// detection by extension). It does not need to point to an existing file.
    pub fn apply_to_text(
        &mut self,
        text: &str,
        patch_str: &str,
        path: &str,
    ) -> Result<PatchResult, HashlineError> {
        let (edits, warnings, _file_op, aborted) = parse_patch(patch_str);

        if edits.is_empty() {
            if aborted {
                let hash = hash::compute_file_hash(text);
                return Ok(PatchResult {
                    applied_edits: 0,
                    hash,
                    warnings,
                    text: text.to_string(),
                    dry_run: false,
                });
            }
            return if warnings.is_empty() {
                Err(HashlineError::EmptyPatch)
            } else {
                Err(HashlineError::EmptyPatchWithReason {
                    reason: warnings[0].clone().into_boxed_str(),
                })
            };
        }

        // Resolve block edits
        let resolved = resolve_block_edits(&edits, text, path, self.block_resolver.as_deref())
            .map_err(|msg| HashlineError::BlockUnresolved {
                line: 0,
                message: msg,
            })?;

        // Build fake FileContent-like data for apply_edits
        let entries: Vec<crate::document::LineEntry> = text
            .split('\n')
            .map(|s| crate::document::LineEntry {
                content: s.to_string(),
                short_hash: hash::short_hash_value(s),
            })
            .collect();

        let mut lines: Vec<String> = split_normalized(text);
        let had_trailing_newline = text.ends_with('\n');
        let path_obj = std::path::Path::new(path);

        crate::commands::patch::apply_edits(&mut lines, &entries, path_obj, &resolved)?;

        let result = if had_trailing_newline && !lines.is_empty() {
            lines.join("\n") + "\n"
        } else if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n")
        };

        let new_hash = hash::compute_file_hash(&result);

        Ok(PatchResult {
            applied_edits: resolved.len(),
            hash: new_hash,
            warnings,
            text: result,
            dry_run: false,
        })
    }

    /// Apply pre-parsed edits to an in-memory text buffer.
    ///
    /// Unlike [`Editor::apply_to_text`], this method takes already-parsed
    /// [`Edit`](crate::types::Edit) items rather than a patch string.
    /// Useful when the consumer has already called `parse_patch` and wants
    /// to apply the same edits to multiple versions of a file (e.g. recovery).
    ///
    /// Like `apply_to_text`, it does **not** touch disk or snapshots.
    /// Block edits are resolved via the configured block resolver.
    pub fn apply_edits(
        &mut self,
        text: &str,
        edits: &[crate::types::Edit],
        path: &str,
    ) -> Result<PatchResult, HashlineError> {
        // Resolve block edits
        let resolved = resolve_block_edits(edits, text, path, self.block_resolver.as_deref())
            .map_err(|msg| HashlineError::BlockUnresolved {
                line: 0,
                message: msg,
            })?;

        let entries: Vec<crate::document::LineEntry> = text
            .split('\n')
            .map(|s| crate::document::LineEntry {
                content: s.to_string(),
                short_hash: hash::short_hash_value(s),
            })
            .collect();

        let mut lines: Vec<String> = split_normalized(text);
        let had_trailing_newline = text.ends_with('\n');
        let path_obj = std::path::Path::new(path);

        crate::commands::patch::apply_edits(&mut lines, &entries, path_obj, &resolved)?;

        let result = if had_trailing_newline && !lines.is_empty() {
            lines.join("\n") + "\n"
        } else if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n")
        };

        let new_hash = hash::compute_file_hash(&result);

        Ok(PatchResult {
            applied_edits: resolved.len(),
            hash: new_hash,
            warnings: Vec::new(),
            text: result,
            dry_run: false,
        })
    }

    // ------------------------------------------------------------------
    // Write
    // ------------------------------------------------------------------

    /// Write content to a file and return its hashline representation.
    ///
    /// `force` controls whether an existing file is overwritten.
    pub fn write(
        &mut self,
        path: &Path,
        content: &str,
        force: bool,
    ) -> Result<WriteResult, HashlineError> {
        if path.exists() && !force {
            return Err(HashlineError::TargetExists {
                path: path.display().to_string(),
            });
        }

        let normalized = crate::normalize::normalize_to_lf(content);
        let bom_result = crate::normalize::strip_bom(&normalized);
        let write_content = bom_result.text;

        crate::commands::common::fast_write(path, write_content.as_bytes())?;

        // Record new snapshot
        if self.config.enable_snapshots {
            self.snapshot_store
                .record(&path.to_string_lossy(), &write_content, None);
        }

        // Re-read for hashline output
        let fc = FileContent::load(path)?;
        let entries = fc.lines_with_hashes();
        let raw_lines = fc.lines();

        let lines: Vec<LineWithHash> = entries
            .iter()
            .enumerate()
            .filter(|(i, entry)| {
                !(entry.content.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline)
            })
            .map(|(i, entry)| LineWithHash {
                n: i + 1,
                hash: hash::format_short_hash(entry.short_hash),
                content: entry.content.clone(),
            })
            .collect();

        Ok(WriteResult {
            path: fc.path,
            hash: fc.hash,
            lines,
        })
    }

    // ------------------------------------------------------------------
    // Find Block
    // ------------------------------------------------------------------

    /// Find the syntactic block containing a given anchor line.
    ///
    /// Uses the configured block resolver. Falls back to the built-in
    /// heuristic resolver if none was explicitly set.
    pub fn find_block(
        &self,
        path: &Path,
        anchor_str: &str,
    ) -> Result<FindBlockResult, HashlineError> {
        let fc = FileContent::load(path)?;
        let entries = fc.lines_with_hashes();

        let parsed = crate::anchor::parse_anchor(anchor_str)?;
        let resolved = crate::anchor::resolve(&parsed, &fc)?;
        let anchor_index = resolved.index;

        // Use configured resolver, or fall back to built-in
        let fallback = crate::builtin_resolver::BuiltinBlockResolver;
        let resolver: &dyn BlockResolverTrait = self.block_resolver.as_deref().unwrap_or(&fallback);

        let (language, start, end) = crate::builtin_resolver::resolve_block_boundaries(
            &entries,
            anchor_index,
            path,
            resolver,
        )?;

        let block_lines: Vec<LineWithHash> = entries[start..=end]
            .iter()
            .enumerate()
            .map(|(i, entry)| LineWithHash {
                n: start + i + 1,
                hash: hash::format_short_hash(entry.short_hash),
                content: entry.content.clone(),
            })
            .collect();

        Ok(FindBlockResult {
            language,
            line_count: fc.len(),
            block_lines,
        })
    }

    // ------------------------------------------------------------------
    // Snapshot management
    // ------------------------------------------------------------------

    /// Invalidate the cached snapshot for a single path.
    pub fn invalidate_snapshot(&mut self, path: &Path) {
        self.snapshot_store.invalidate(&path.to_string_lossy());
    }

    /// Clear all cached snapshots.
    pub fn clear_snapshots(&mut self) {
        self.snapshot_store.clear();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn split_normalized(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') && parts.last() == Some(&"") {
        parts.pop();
    }
    parts.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_store::InMemorySnapshotStore;
    use std::fs;
    use tempfile::TempDir;

    fn temp_file(content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.rs");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn test_read_returns_lines_with_hashes() {
        let (_d, path) = temp_file("fn main() {\n    let x = 1;\n}\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store).with_builtin_resolver();

        let result = ed.read(&path).unwrap();
        assert_eq!(result.lines.len(), 3);
        assert_eq!(result.lines[0].content, "fn main() {");
        assert_eq!(result.lines[0].hash.len(), 2);
    }

    #[test]
    fn test_patch_applies_swap() {
        let (_d, path) = temp_file("line1\nline2\nline3\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);

        let result = ed.patch(&path, "SWAP 2:\n+replaced").unwrap();
        assert_eq!(result.applied_edits, 2);

        let readback = fs::read_to_string(&path).unwrap();
        assert_eq!(readback, "line1\nreplaced\nline3\n");
    }

    #[test]
    fn test_dry_run_does_not_modify_file() {
        let (_d, path) = temp_file("line1\nline2\nline3\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);
        let original = fs::read_to_string(&path).unwrap();

        let result = ed.dry_run(&path, "SWAP 2:\n+replaced").unwrap();
        assert!(result.dry_run);
        assert!(result.text.contains("replaced"));

        let unchanged = fs::read_to_string(&path).unwrap();
        assert_eq!(unchanged, original);
    }

    #[test]
    fn test_noop_loop_guard_fires_after_limit() {
        // A patch that targets a line whose content is already the payload
        // produces no net change. Repeating the SAME patch 3x must surface
        // NoopLoop, not silently succeed.
        let (_d, path) = temp_file("line1\nline2\nline3\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);

        // SWAP 2 to the same content — net no-op.
        let noop_patch = "SWAP 2:\n+line2";
        ed.patch(&path, noop_patch).unwrap();
        ed.patch(&path, noop_patch).unwrap();
        let err = ed.patch(&path, noop_patch).unwrap_err();
        assert!(
            err.to_string().contains("no-op loop"),
            "expected NoopLoop error, got: {err}"
        );
    }

    #[test]
    fn test_noop_guard_does_not_block_real_edits() {
        let (_d, path) = temp_file("line1\nline2\nline3\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);

        // Same patch that DOES change content, applied repeatedly: first apply
        // changes, the rest are no-ops relative to the new content but the
        // guard fires only on identical repeated no-ops of the SAME text.
        // The first apply is a real change (resets streak).
        ed.patch(&path, "SWAP 2:\n+replaced").unwrap();
        // Re-applying the same patch to the already-replaced file is a no-op:
        // streak 1, then 2 (both Ok), then 3 → NoopLoop.
        ed.patch(&path, "SWAP 2:\n+replaced").unwrap();
        ed.patch(&path, "SWAP 2:\n+replaced").unwrap();
        let err = ed.patch(&path, "SWAP 2:\n+replaced").unwrap_err();
        assert!(
            err.to_string().contains("no-op loop"),
            "expected NoopLoop on 3rd identical no-op, got: {err}"
        );
        let readback = fs::read_to_string(&path).unwrap();
        assert_eq!(readback, "line1\nreplaced\nline3\n");
    }

    #[test]
    fn test_phase3_recovery_on_external_shift() {
        // Phase 3: read records a snapshot; an external edit shifts the target
        // line; a patch anchored to the OLD snapshot's hashes should recover
        // via 3-way merge instead of failing stale.
        let (_d, path) = temp_file("alpha\nbeta\ngamma\n");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);

        // Read → records snapshot (hash of "beta" at line 2).
        let read = ed.read(&path).unwrap();
        let beta_hash = &read.lines[1].hash; // line 2 = "beta"

        // External edit: insert a line above "beta" (now "beta" is line 3).
        fs::write(&path, "alpha\ninserted\nbeta\ngamma\n").unwrap();

        // Patch anchored to the OLD hash (line 2, but content now at line 3).
        // The anchor hash no longer matches line 2 → recovery should kick in.
        let patch = format!("SWAP 2:{beta_hash}:\n+replaced");
        let result = ed.patch(&path, &patch).unwrap();
        assert!(result.text.contains("replaced"), "recovery should apply the swap");
        let readback = fs::read_to_string(&path).unwrap();
        assert_eq!(readback, "alpha\ninserted\nreplaced\ngamma\n");
    }

    #[test]
    fn test_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.rs");
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::with_store(store);

        let result = ed.write(&path, "fn hello() {}", false).unwrap();
        assert_eq!(result.lines.len(), 1);

        assert!(path.exists());
    }

    #[test]
    fn test_write_without_force_fails_on_existing() {
        let (_d, path) = temp_file("existing\n");
        let mut ed = Editor::without_snapshots();
        let err = ed.write(&path, "new", false).unwrap_err();
        assert!(matches!(err, HashlineError::TargetExists { .. }));
    }

    #[test]
    fn test_write_with_force_overwrites() {
        let (_d, path) = temp_file("original\n");
        let mut ed = Editor::without_snapshots();
        let result = ed.write(&path, "replaced", true).unwrap();
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].content, "replaced");
    }

    #[test]
    fn test_block_resolutions() {
        use crate::hash::{format_short_hash, short_hash_value};
        let content = "fn hello() {\n    let x = 1;\n}\nfn world() {\n    let y = 2;\n}\n";
        let (_d, path) = temp_file(content);
        let ed = Editor::without_snapshots().with_builtin_resolver();

        let hash = format_short_hash(short_hash_value("fn hello() {"));
        let result = ed.find_block(&path, &format!("1:{hash}")).unwrap();
        assert!(result.block_lines.len() >= 3);
    }

    #[test]
    fn test_patch_returns_hash() {
        let (_d, path) = temp_file("aaa\nbbb\nccc\n");
        let mut ed = Editor::without_snapshots();

        let result = ed.patch(&path, "SWAP 2:\n+xxx").unwrap();
        // Hash is a 4-hex string
        assert_eq!(result.hash.len(), 4);
    }

    #[test]
    fn test_stale_anchor_is_detected() {
        // If we read, modify the file externally, then patch,
        // hashline should detect the stale anchor.
        let (_d, path) = temp_file("line1\noriginal\nline3\n");
        let mut ed = Editor::without_snapshots();

        // Change the file externally
        fs::write(&path, "line1\nmodified\nline3\n").unwrap();

        let result = ed.patch(&path, "SWAP 2:47:\n+xxx");
        // This should error because line 2's hash changed
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error, got: {err}"
        );
    }

    #[test]
    fn test_empty_patch_errors() {
        let (_d, path) = temp_file("aaa\nbbb\nccc\n");
        let mut ed = Editor::without_snapshots();
        let err = ed.patch(&path, "").unwrap_err();
        assert!(matches!(err, HashlineError::EmptyPatch));
    }

    #[test]
    fn test_invalidate_and_clear() {
        let store = InMemorySnapshotStore::new();
        let mut ed = Editor::new(store, HashlineConfig::default());
        let (_d, path) = temp_file("data\n");

        let _ = ed.read(&path).unwrap();
        assert!(ed.snapshot_store().head(&path.to_string_lossy()).is_some());

        ed.invalidate_snapshot(&path);
        assert!(ed.snapshot_store().head(&path.to_string_lossy()).is_none());

        let _ = ed.read(&path).unwrap();
        assert!(ed.snapshot_store().head(&path.to_string_lossy()).is_some());

        ed.clear_snapshots();
        assert!(ed.snapshot_store().head(&path.to_string_lossy()).is_none());
    }
}
