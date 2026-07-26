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
use crate::types::BlockResolver as BlockResolverTrait;

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
}

impl Editor {
    /// Create an `Editor` with a custom snapshot store and default config.
    pub fn with_store(store: impl SnapshotStore + 'static) -> Self {
        Self {
            snapshot_store: Box::new(store),
            block_resolver: None,
            config: HashlineConfig::default(),
        }
    }

    /// Create an `Editor` with a custom store and config.
    pub fn new(store: impl SnapshotStore + 'static, config: HashlineConfig) -> Self {
        Self {
            snapshot_store: Box::new(store),
            block_resolver: None,
            config,
        }
    }

    /// Convenience: create an `Editor` with no snapshot caching
    /// (equivalent to `NoopSnapshotStore`).
    pub fn without_snapshots() -> Self {
        Self {
            snapshot_store: Box::new(crate::snapshot_store::NoopSnapshotStore),
            block_resolver: None,
            config: HashlineConfig {
                enable_snapshots: false,
                ..HashlineConfig::default()
            },
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

        // Apply edits to in-memory lines
        let mut lines: Vec<String> = split_normalized(text);
        let entries = fc.lines_with_hashes();
        let had_trailing_newline = fc.trailing_newline;

        crate::commands::patch::apply_edits(&mut lines, &entries, path, &resolved)?;

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
