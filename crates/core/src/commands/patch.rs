use std::io::Write;
use std::path::Path;

use crate::cli::PatchCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::normalize::{detect_line_ending, restore_line_endings};
use crate::parser::parse_patch;
use crate::types::{
    BlockMode, ChangeSet, ChangedLine, Clipboard, Cursor, Edit, FileOp, InsertMode,
};

/// Cell budget for the LCS dynamic-programming table used by the changeset
/// and dry-run diffs (`m * n` cells). Past it the diff falls back to
/// unique-line anchors, then to a wholesale delete+insert — quadratic time
/// and memory are never paid on huge files.
const MAX_LCS_CELLS: usize = 4_000_000;

thread_local! {
    /// Warnings emitted while *applying* edits (boundary-echo repairs,
    /// delimiter warnings). The CLI prints them to stderr at emit time via
    /// [`emit_apply_warning`]; embedders that cannot see stderr (the MCP
    /// server, whose stdout is the JSON-RPC channel) drain them with
    /// [`take_apply_warnings`] and surface them in the tool result.
    static APPLY_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record an apply-time warning: echo it to stderr for CLI users and queue
/// it for embedders that drain [`take_apply_warnings`].
fn emit_apply_warning(warning: &str) {
    eprintln!("warning: {warning}");
    APPLY_WARNINGS.with(|sink| sink.borrow_mut().push(warning.to_string()));
}

/// Drain warnings emitted during patch application since the last call.
/// Callers that surface apply results to a non-stderr channel (MCP) should
/// drain once *before* applying (to drop stale entries from a failed apply)
/// and once after.
pub fn take_apply_warnings() -> Vec<String> {
    APPLY_WARNINGS.with(|sink| std::mem::take(&mut *sink.borrow_mut()))
}

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: PatchCmd,
) -> Result<(), HashlineError> {
    // Reject device/reserved paths before any filesystem probe —
    // `metadata`/`read` on a DOS device name (CON, AUX, ...) can block.
    crate::commands::common::reject_special_write_path(&cmd.file)?;

    // Resolve patch content: `-` reads stdin, `@path` reads file, otherwise use literal.
    let patch_content = resolve_patch_content(&cmd.patch)?;

    let fc = FileContent::load(&cmd.file)?;
    let text = &fc.normalized;
    let (edits, warnings, file_op, aborted) = parse_patch(&patch_content);

    // Surface parser warnings even when no edits were produced so callers
    // can tell a syntactically-broken patch from an empty one.
    for w in &warnings {
        eprintln!("warning: {w}");
    }

    // File-level ops (`REM` / `MV <dest>`). These used to be parsed and then
    // silently discarded — the patch reported success while nothing
    // happened. Conservative semantics (documented in CHANGELOG): a patch
    // may contain EITHER line edits OR a single file op, never both; the
    // file op runs only after the patch parses cleanly.
    if let Some(op) = &file_op {
        if aborted || !edits.is_empty() {
            return Err(HashlineError::PatchRejected {
                reason: "a file-level operation (REM/MV) cannot be combined with \
                         line edits or an aborted patch; use `hashline remove` / \
                         `hashline rename` for file operations"
                    .into(),
            });
        }
        return apply_patch_file_op(ctx, &cmd.file, op);
    }

    if edits.is_empty() {
        if aborted {
            // The patch was rejected atomically — it mixed a valid operation
            // with an unknown/malformed one, or carried an explicit abort
            // marker. Nothing was written, so report failure: exiting 0 here
            // tells the caller the edit landed when it did not.
            let reason = warnings
                .first()
                .map(|w| w.lines().next().unwrap_or(w).to_string())
                .unwrap_or_else(|| {
                    "the patch was rejected before any edits were applied".to_string()
                });
            return Err(HashlineError::PatchRejected {
                reason: reason.into_boxed_str(),
            });
        }
        if warnings.is_empty() {
            return Err(HashlineError::EmptyPatch);
        }
        // Include the first warning in the error message so callers get
        // context about what went wrong, not just "empty patch".
        return Err(HashlineError::EmptyPatchWithReason {
            reason: warnings[0].clone().into_boxed_str(),
        });
    }

    // Split on newlines. Drop the trailing empty segment that split('\n')
    // produces when a file ends with '\n' — we add it back on join.
    let mut lines: Vec<String> = split_normalized(text);
    let had_trailing_newline = fc.trailing_newline;

    // Per-line short hashes are only read when an edit carries an
    // `expected_hash` (`N:hh` anchors); bare `N` anchors never consult them.
    // Skip the whole-file xxh32 pass when the patch has none — the entries
    // still carry content for block resolution and `CUT` capture.
    let entries = if edits_need_hashes(&edits) {
        fc.lines_with_hashes()
    } else {
        fc.lines_without_hashes()
    };
    // Opt-in (issue #119): also hand back fresh anchors so the caller can
    // chain a follow-up edit without an extra `read` round-trip. Flag or
    // `HASHLINE_RETURN_ANCHORS=1`. Off by default to keep output token-minimal.
    // Resolved before apply so the changeset knows whether to pay for the
    // full-file anchor listing.
    let emit_anchors = cmd.emit_anchors || emit_anchors_from_env();
    // Dry-run only needs the applied lines for `format_diff` — skip the
    // changeset diff, result-file hash, and anchor listing entirely.
    let changeset = if cmd.dry_run {
        apply_edits(&mut lines, &entries, &cmd.file, &edits)?;
        None
    } else {
        // Count unique high-level operations (each has a distinct line_num).
        let op_count = count_operations(&edits);
        Some(apply_edits_tracked(
            &mut lines,
            &entries,
            &cmd.file,
            &edits,
            op_count,
            had_trailing_newline,
            emit_anchors,
        )?)
    };

    let result = if had_trailing_newline && !lines.is_empty() {
        lines.join("\n") + "\n"
    } else if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };
    let line_ending = detect_line_ending(&fc.raw);
    // A mixed-EOL file is rewritten with the dominant ending throughout —
    // per-line endings are not preserved. Warn loudly instead of silently
    // canonicalizing (bd-write-safety-batch-sz62).
    if crate::normalize::has_mixed_line_endings(&fc.raw) {
        eprintln!(
            "warning: file has mixed line endings; write-back uses the dominant \
             {:?} ending for every line",
            line_ending
        );
    }
    // Restore the file's dominant line ending (LF/CRLF/CR) and re-attach the
    // BOM that `FileContent::load` stripped, so a patch round-trips the
    // file's encoding conventions instead of silently rewriting them.
    let mut final_text = restore_line_endings(&result, line_ending);
    if !fc.bom.is_empty() {
        final_text.insert_str(0, &fc.bom);
    }

    if cmd.dry_run {
        // Always diff the LF-normalized, BOM-free `result` against
        // `fc.normalized` — `final_text` now carries the file's EOL style
        // and BOM, which would show up as spurious diff lines (Bug #101).
        let diff_text = &result;
        if cmd.json {
            // Return JSON diff info instead of text diff
            let diff_lines = format_diff(&fc.normalized, diff_text);
            let payload = serde_json::json!({
                "success": true,
                "file": cmd.file.display().to_string(),
                "dry_run": true,
                "edits_applied": count_operations(&edits),
                "diff": diff_lines,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&payload)?)?;
        } else {
            // Show a unified-diff-alike snippet instead of the entire file.
            let original_text = &fc.normalized;
            let diff_lines = format_diff(original_text, diff_text);
            for dl in &diff_lines {
                writeln!(ctx.stdout(), "{dl}")?;
            }
        }
        return Ok(());
    }

    // Past the dry-run early return a changeset is always present.
    let changeset = changeset.expect("changeset computed for non-dry-run patch");

    // TOCTOU / lost-update guard: the edit was computed against `fc`, but
    // the write below targets the path, not the loaded content. If another
    // process swapped or rewrote the file since the load, writing would
    // silently destroy their change (or, for a swapped-in decoy, destroy an
    // unrelated file). Re-read and compare content; fail closed on drift.
    // `write_patch_output` additionally re-checks drift inside the atomic
    // persist loop (--safe) and byte-verifies the result afterwards, so a
    // concurrent overwrite is reported as CONCURRENT_MODIFICATION instead
    // of a silent OK.
    write_patch_output(&cmd.file, &final_text, &fc, cmd.safe)?;

    // Render output based on output mode.
    match ctx.output_mode() {
        crate::context::OutputMode::Compact => {
            // Agent-native: OK header + changed lines only
            let path = crate::output::escape_compact_field(&cmd.file.display().to_string());
            let changed_count =
                changeset.modified.len() + changeset.inserted.len() + changeset.deleted.len();
            writeln!(
                ctx.stdout(),
                "OK {}#{} edits={} changed={}",
                path,
                changeset.file_hash,
                changeset.edits_applied,
                changed_count,
            )?;
            for line in &changeset.modified {
                writeln!(
                    ctx.stdout(),
                    "~{}:{}|{}",
                    line.line,
                    line.hash,
                    line.content
                )?;
            }
            for line in &changeset.inserted {
                writeln!(
                    ctx.stdout(),
                    "+{}:{}|{}",
                    line.line,
                    line.hash,
                    line.content
                )?;
            }
            for &line_num in &changeset.deleted {
                writeln!(ctx.stdout(), "-{}", line_num)?;
            }
            if emit_anchors {
                write_updated_anchors(ctx.stdout(), &cmd.file.display().to_string(), &changeset)?;
            }
        }
        crate::context::OutputMode::Verbose => {
            // Human-readable: full file dump (old default)
            writeln!(
                ctx.stdout(),
                "[{}#{}]",
                cmd.file.display(),
                changeset.file_hash
            )?;
            use crate::hash::write_short_hash_bytes;
            let mut hash_buf = [0u8; 2];
            for (i, content) in lines.iter().enumerate() {
                let short = crate::hash::short_hash_value_indexed(content, i + 1);
                write_short_hash_bytes(&mut hash_buf, short);
                let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
                writeln!(ctx.stdout(), "{}:{}|{}", i + 1, hash_str, content)?;
            }
        }
        crate::context::OutputMode::Json => {
            // Structured JSON with changed lines
            let changed: Vec<serde_json::Value> = changeset
                .modified
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "type": "modified",
                        "line": l.line,
                        "hash": l.hash,
                        "content": l.content,
                    })
                })
                .chain(changeset.inserted.iter().map(|l| {
                    serde_json::json!({
                        "type": "inserted",
                        "line": l.line,
                        "hash": l.hash,
                        "content": l.content,
                    })
                }))
                .chain(changeset.deleted.iter().map(|&n| {
                    serde_json::json!({
                        "type": "deleted",
                        "line": n,
                    })
                }))
                .collect();
            if emit_anchors {
                // `updated_anchors`: full fresh anchor listing in `read`
                // JSON shape ({n, hash, content}) so agents can chain edits
                // without an extra `read` call (issue #119). Streamed
                // piecewise — it is a whole-file listing, and a
                // Vec<serde_json::Value> for it cost ~1GB of peak RSS on
                // large files (same fix as `read --json`, commit 6adfc1a).
                // serde_json's Map is a BTreeMap, so keys serialize in
                // sorted order; the byte layout below matches
                // `to_string` of the equivalent `json!` object exactly.
                let out = ctx.stdout();
                write!(out, "{{\"changed\":")?;
                serde_json::to_writer(&mut *out, &changed)?;
                write!(out, ",\"edits_applied\":{}", changeset.edits_applied)?;
                write!(out, ",\"file\":")?;
                serde_json::to_writer(&mut *out, &cmd.file.display().to_string())?;
                write!(out, ",\"hash\":")?;
                serde_json::to_writer(&mut *out, &changeset.file_hash)?;
                write!(out, ",\"success\":true,\"updated_anchors\":[")?;
                for (idx, l) in changeset.updated_anchors.iter().enumerate() {
                    if idx > 0 {
                        out.write_all(b",")?;
                    }
                    write!(out, "{{\"content\":")?;
                    serde_json::to_writer(&mut *out, &l.content)?;
                    write!(out, ",\"hash\":")?;
                    serde_json::to_writer(&mut *out, &l.hash)?;
                    write!(out, ",\"n\":{}}}", l.line)?;
                }
                writeln!(out, "]}}")?;
            } else {
                let payload = serde_json::json!({
                    "success": true,
                    "file": cmd.file.display().to_string(),
                    "hash": changeset.file_hash,
                    "edits_applied": changeset.edits_applied,
                    "changed": changed,
                });
                writeln!(ctx.stdout(), "{}", serde_json::to_string(&payload)?)?;
            }
        }
        crate::context::OutputMode::Ndjson => {
            // Same as compact for now
            let path = crate::output::escape_compact_field(&cmd.file.display().to_string());
            let changed_count =
                changeset.modified.len() + changeset.inserted.len() + changeset.deleted.len();
            writeln!(
                ctx.stdout(),
                "OK {}#{} edits={} changed={}",
                path,
                changeset.file_hash,
                changeset.edits_applied,
                changed_count,
            )?;
            for line in &changeset.modified {
                writeln!(
                    ctx.stdout(),
                    "~{}:{}|{}",
                    line.line,
                    line.hash,
                    line.content
                )?;
            }
            for line in &changeset.inserted {
                writeln!(
                    ctx.stdout(),
                    "+{}:{}|{}",
                    line.line,
                    line.hash,
                    line.content
                )?;
            }
            for &line_num in &changeset.deleted {
                writeln!(ctx.stdout(), "-{}", line_num)?;
            }
            if emit_anchors {
                write_updated_anchors(ctx.stdout(), &cmd.file.display().to_string(), &changeset)?;
            }
        }
    }

    Ok(())
}

/// True when `HASHLINE_RETURN_ANCHORS` is set to a truthy value
/// (`1`, `true`, `yes`, `on`). Lets agents opt in without touching argv.
pub fn emit_anchors_from_env() -> bool {
    match std::env::var("HASHLINE_RETURN_ANCHORS") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Write the full fresh anchor listing from the changeset in `read` output
/// format so a follow-up patch can reuse the anchors directly
/// (`[path#HASH]` + `N:hh|content`). Hashes come from
/// [`ChangeSet::updated_anchors`], computed with the same position-seeded
/// hash the `read` path uses.
fn write_updated_anchors<W: Write>(
    stdout: &mut W,
    path: &str,
    changeset: &ChangeSet,
) -> std::io::Result<()> {
    writeln!(
        stdout,
        "[{}#{}]",
        crate::output::escape_compact_field(path),
        changeset.file_hash
    )?;
    for line in &changeset.updated_anchors {
        writeln!(stdout, "{}:{}|{}", line.line, line.hash, line.content)?;
    }
    Ok(())
}

/// Write-time stale-file guard shared by every patch write path (CLI,
/// MCP, `Editor::patch`). `loaded` is the [`FileContent`] the edits were
/// computed against; the file on disk is re-read and compared. Any drift —
/// a rewrite, a truncate, or a swapped-in decoy — fails closed with
/// [`HashlineError::StaleFile`] instead of clobbering the other writer's
/// change.
///
/// Residual race: a writer landing between this check and the actual write
/// is still possible — closing it fully needs OS-level locking (or a
/// compare-and-swap write), which hashline deliberately does not take.
/// This narrows the window from "whole patch apply" to a single re-read.
/// [`write_patch_output`] shrinks it further: `--safe` re-runs this check
/// inside the persist retry loop, and every mode byte-verifies the file
/// after writing.
pub(crate) fn reject_if_source_drifted(
    path: &Path,
    loaded: &FileContent,
) -> Result<(), HashlineError> {
    let fresh = FileContent::load(path)?;
    // Compare the RAW on-disk text, not the normalized form: `normalized`
    // is LF/BOM-stripped, so a BOM add/remove, an EOL-style flip, or a
    // trailing-newline change produced identical normalized text and the
    // drift went unnoticed — the patch then rewrote the file with its own
    // conventions, silently discarding the other writer's encoding change
    // (bd-write-safety-batch-sz62).
    if fresh.raw != loaded.raw {
        return Err(HashlineError::StaleFile {
            path: path.display().to_string(),
        });
    }
    Ok(())
}

/// Write a computed patch result to `path` with drift protection and
/// post-write verification. Shared by the CLI `patch` command, the MCP
/// `patch` tool, and `Editor::patch`.
///
/// Sequence:
/// 1. [`reject_if_source_drifted`] — refuse to clobber a file that changed
///    since `fc` was loaded.
/// 2. Write: `atomic_write_with_check` for `--safe` (the drift check runs
///    again inside the persist retry loop, so a swap during temp-write or
///    persist backoff is caught before the rename), `fast_write` otherwise.
/// 3. [`verify_written_bytes`] — byte-exact re-read. A mismatch means
///    another process overwrote us between write and verify (or the write
///    itself was intercepted); fail with
///    [`HashlineError::ConcurrentModification`] instead of reporting OK
///    for content that is no longer on disk.
///
/// Residual: without OS-level locking a writer can still land in the
/// nanoseconds between the last drift check and the rename, or between a
/// fast write and the verify — that writer wins and *this* command reports
/// the failure, so no silent OK survives. What cannot be detected is a
/// writer whose content lands and is itself overwritten before anyone
/// verifies; see the `fast_write` docs for the torn-read caveat.
pub(crate) fn write_patch_output(
    path: &Path,
    final_text: &str,
    fc: &FileContent,
    safe: bool,
) -> Result<(), HashlineError> {
    let bytes = final_text.as_bytes();
    reject_if_source_drifted(path, fc)?;
    if safe {
        crate::commands::common::atomic_write_with_check(
            path,
            |file| file.write_all(bytes),
            || reject_if_source_drifted(path, fc),
        )?;
    } else {
        crate::commands::common::fast_write(path, bytes)?;
    }
    crate::commands::common::verify_written_bytes(path, bytes)
}

/// Execute a patch that consists solely of a file-level operation (`REM` or
/// `MV <dest>`). Mirrors the dedicated `remove`/`rename` commands' guards —
/// existence, directory rejection, same-file and destination-exists checks —
/// but with no `--force`: a `MV` onto an existing path fails rather than
/// deleting it. A relative `MV` destination resolves against the source
/// file's parent directory (the section the op was written for), not the
/// process cwd.
fn apply_patch_file_op<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &Path,
    op: &FileOp,
) -> Result<(), HashlineError> {
    match op {
        FileOp::Remove => {
            if !file.exists() {
                return Err(HashlineError::FileNotFound {
                    path: file.display().to_string(),
                });
            }
            std::fs::remove_file(file)?;
            match ctx.output_mode() {
                crate::context::OutputMode::Compact | crate::context::OutputMode::Ndjson => {
                    writeln!(
                        ctx.stdout(),
                        "OK {}",
                        crate::output::escape_compact_field(&file.display().to_string())
                    )?;
                }
                crate::context::OutputMode::Verbose => {
                    writeln!(ctx.stdout(), "removed '{}'", file.display())?;
                }
                crate::context::OutputMode::Json => {
                    let output = serde_json::json!({
                        "success": true,
                        "path": file.display().to_string(),
                        "file_op": "remove",
                    });
                    writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
                }
            }
        }
        FileOp::Rename(dest) => {
            let dst = {
                let d = Path::new(dest);
                if d.is_absolute() {
                    d.to_path_buf()
                } else {
                    match file.parent() {
                        Some(p) if !p.as_os_str().is_empty() => p.join(d),
                        _ => d.to_path_buf(),
                    }
                }
            };
            if !file.exists() {
                return Err(HashlineError::FileNotFound {
                    path: file.display().to_string(),
                });
            }
            let src_meta = std::fs::metadata(file)?;
            if src_meta.is_dir() {
                return Err(HashlineError::IsDirectory {
                    path: file.display().to_string(),
                });
            }
            if !src_meta.is_file() {
                return Err(HashlineError::InvalidPath {
                    path: file.display().to_string(),
                    reason: "source is not a regular file".into(),
                });
            }
            if crate::commands::common::paths_refer_to_same_file(file, &dst) {
                return Err(HashlineError::SameFile {
                    path: file.display().to_string(),
                });
            }
            if dst.exists() {
                return Err(HashlineError::TargetExists {
                    path: dst.display().to_string(),
                });
            }
            if let Some(parent) = dst.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::rename(file, &dst)?;
            match ctx.output_mode() {
                crate::context::OutputMode::Compact | crate::context::OutputMode::Ndjson => {
                    writeln!(
                        ctx.stdout(),
                        "OK {}>{}",
                        crate::output::escape_compact_field(&file.display().to_string()),
                        crate::output::escape_compact_field(&dst.display().to_string())
                    )?;
                }
                crate::context::OutputMode::Verbose => {
                    writeln!(
                        ctx.stdout(),
                        "renamed '{}' -> '{}'",
                        file.display(),
                        dst.display()
                    )?;
                }
                crate::context::OutputMode::Json => {
                    let output = serde_json::json!({
                        "success": true,
                        "src": file.display().to_string(),
                        "dst": dst.display().to_string(),
                        "file_op": "rename",
                    });
                    writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
                }
            }
        }
    }
    Ok(())
}

/// Resolve the `<PATCH>` argument: `-` reads stdin, `@path` reads file, otherwise literal.
fn resolve_patch_content(patch: &str) -> Result<String, HashlineError> {
    if patch == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(HashlineError::Io)?;
        return Ok(buf);
    }
    if let Some(path_str) = patch.strip_prefix('@') {
        let path = Path::new(path_str);
        return std::fs::read_to_string(path).map_err(|e| {
            HashlineError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("failed to read patch file '{}': {e}", path.display()),
            ))
        });
    }
    Ok(patch.to_owned())
}

/// Produce a minimal unified-diff snippet showing only changed lines,
/// suitable for dry-run review. Loosely follows `diff -u` style but omits
/// the timestamp header.
///
/// The alignment reuses [`changeset_diff_ops`] — common prefix/suffix
/// trim, exact LCS under a cell budget, patience-style unique-line anchors
/// for sparse edits in huge files, and a wholesale delete+insert fallback —
/// so dry-run costs the same bounded diff the changeset path pays instead
/// of the old `vec![vec![0; n+1]; m+1]` table that OOM'd on ~100k-line
/// files. Emitted `+`/`-` rows are additionally capped so a wholesale
/// rewrite cannot flood the caller.
pub(crate) fn format_diff(original: &str, final_text: &str) -> Vec<String> {
    const MAX_DIFF_ROWS: usize = 2_000;
    const MAX_DIFF_BYTES: usize = 1024 * 1024;

    let left: Vec<&str> = if original.is_empty() {
        vec![]
    } else {
        original.split('\n').collect()
    };
    let right: Vec<&str> = if final_text.is_empty() {
        vec![]
    } else {
        final_text.split('\n').collect()
    };
    if left == right {
        return vec!["(no changes)".into()];
    }
    let ops = changeset_diff_ops(&left, &right, MAX_LCS_CELLS);
    let mut out: Vec<String> = vec!["@@ -- ++ @@".into()];
    let mut bytes_out = 0usize;
    let mut emitted = 0usize;
    let mut omitted = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    let mut push_row = |tag: char, line: &str, out: &mut Vec<String>| {
        // `format!` allocates the row up front; check the budget against
        // the would-be length so a single giant line cannot slip past the
        // byte cap.
        if emitted >= MAX_DIFF_ROWS || bytes_out + line.len() + 2 > MAX_DIFF_BYTES {
            omitted += 1;
            return;
        }
        let row = format!("{tag}{line}");
        bytes_out += row.len() + 1;
        out.push(row);
        emitted += 1;
    };
    for op in ops {
        match op {
            crate::merge::DiffOp::Equal(count) => {
                i += count;
                j += count;
            }
            crate::merge::DiffOp::Delete(count) => {
                for _ in 0..count {
                    push_row('-', left[i], &mut out);
                    i += 1;
                }
            }
            crate::merge::DiffOp::Insert(count) => {
                for _ in 0..count {
                    push_row('+', right[j], &mut out);
                    j += 1;
                }
            }
        }
    }
    if omitted > 0 {
        out.push(format!("... {omitted} more changed lines not shown ..."));
    }
    out
}

fn split_normalized(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = text.split('\n').collect();
    // Drop trailing empty from split when file ends with '\n'
    if text.ends_with('\n') && parts.last() == Some(&"") {
        parts.pop();
    }
    parts.iter().map(|s| s.to_string()).collect()
}

/// Pure adapter: apply `edits` to `text` and return an [`ApplyResult`].
///
/// This is the bridge the snapshot-based recovery (Phase 3) needs — it turns
/// the mutable `Vec<String>` applier into a pure `text → ApplyResult`
/// function, so edits anchored to a snapshot can be replayed against that
/// snapshot's text (whose hashes match) and then 3-way-merged onto live
/// content. The normal patch path continues to use [`apply_edits`] directly.
///
/// `path` is used only for block resolution / diagnostics.
pub fn apply_edits_pure(
    text: &str,
    edits: &[Edit],
    path: &Path,
) -> Result<crate::types::ApplyResult, HashlineError> {
    let mut lines: Vec<String> = split_normalized(text);
    // Build entries exactly like `FileContent::lines_with_hashes`: keep the
    // phantom trailing empty segment (`text.split('\n')` without the
    // `split_normalized` drop). The applier derives `visible_lines` by
    // trimming ONE trailing empty entry — with the phantom kept, `a\n\n`
    // yields 2 visible lines here exactly as it does on the read/patch
    // path. Without it the pure path saw 1 visible line, rejected anchors
    // the real path accepts, and recovery replay silently returned `None`
    // (bd-mcp-serve-parity-batch-nh8w).
    let entries: Vec<crate::document::LineEntry> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n')
            .enumerate()
            .map(|(i, s)| crate::document::LineEntry {
                content: s.to_string(),
                short_hash: crate::hash::short_hash_value_indexed(s, i + 1),
            })
            .collect()
    };

    apply_edits(&mut lines, &entries, path, edits)?;

    let result = if text.ends_with('\n') && !lines.is_empty() {
        lines.join("\n") + "\n"
    } else if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };

    // Surface apply-time warnings (boundary-echo repairs, delimiter notes)
    // and the first changed line instead of dropping them — recovery
    // consumes both fields. `block_resolutions` stays empty: no producer
    // populates it on any apply path today.
    Ok(crate::types::ApplyResult {
        first_changed_line: first_changed_line_between(text, &result),
        text: result,
        warnings: take_apply_warnings(),
        block_resolutions: Vec::new(),
    })
}

/// First 1-indexed line at which `before` and `after` diverge, or `None`
/// when they are identical. Mirrors `recovery::find_first_changed_line`.
fn first_changed_line_between(before: &str, after: &str) -> Option<usize> {
    if before == after {
        return None;
    }
    let a: Vec<&str> = before.split('\n').collect();
    let b: Vec<&str> = after.split('\n').collect();
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            return Some(i + 1);
        }
    }
    None
}

/// True when any edit in the patch carries a hash-qualified anchor
/// (`N:hh`). The applier validates `expected_hash` against the snapshot's
/// per-line short hashes; when no edit has one, those hashes are never
/// read and computing them is wasted work on large files.
fn edits_need_hashes(edits: &[Edit]) -> bool {
    edits.iter().any(|e| match e {
        Edit::Insert { expected_hash, .. }
        | Edit::Delete { expected_hash, .. }
        | Edit::Block { expected_hash, .. }
        | Edit::Cut { expected_hash, .. } => expected_hash.is_some(),
        Edit::Paste { .. } => false,
    })
}

/// Count the high-level operations in a parsed patch.
///
/// One operation can expand into several [`Edit`] variants — a `SWAP` with a
/// three-line body yields one `Insert` per body line plus a `Delete` — so the
/// operation count is the number of distinct `line_num` ownership keys. The
/// CLI and the MCP server must agree on this number, otherwise the same patch
/// reports a different `edits=` depending on which interface ran it.
pub fn count_operations(edits: &[Edit]) -> usize {
    edits
        .iter()
        .map(|e| match e {
            Edit::Insert { line_num, .. }
            | Edit::Delete { line_num, .. }
            | Edit::Block { line_num, .. }
            | Edit::Cut { line_num, .. }
            | Edit::Paste { line_num, .. } => *line_num,
        })
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Apply parsed edits to a mutable lines vector.
///
/// Each [`Edit`] references original 1-indexed line numbers from the
/// read snapshot (`entries`).  As the live `lines` buffer is mutated by
/// earlier edits, later edits must adjust their target position for the
/// cumulative shift — inserts add lines, deletes remove them.  The
/// `deleted` and `shift` arrays track this per-original-line so every
/// op resolves to the correct live index.
pub fn apply_edits(
    lines: &mut Vec<String>,
    entries: &[crate::document::LineEntry],
    path: &Path,
    edits: &[Edit],
) -> Result<(), HashlineError> {
    apply_edits_with_clipboard(lines, entries, path, edits, &mut Clipboard::default())
}

/// Like `apply_edits` but also returns a [`ChangeSet`] describing what changed.
/// The changeset is computed from the edit operations and the before/after state.
///
/// `want_anchors` controls the opt-in full-file fresh-anchor listing
/// (`ChangeSet::updated_anchors`): it costs one clone+hash per result line,
/// so callers that will not emit it (the default compact/JSON output, and
/// every dry-run) pass `false` and get an empty vec.
pub fn apply_edits_tracked(
    lines: &mut Vec<String>,
    entries: &[crate::document::LineEntry],
    path: &Path,
    edits: &[Edit],
    edits_count: usize,
    trailing_newline: bool,
    want_anchors: bool,
) -> Result<ChangeSet, HashlineError> {
    apply_edits(lines, entries, path, edits)?;
    let changeset = compute_changeset(
        entries,
        lines,
        edits_count,
        path,
        trailing_newline,
        want_anchors,
    );
    Ok(changeset)
}

/// Compute a [`ChangeSet`] by comparing original entries against the final lines.
/// Uses the original entries as the pre-edit snapshot and the final lines as
/// the post-edit result. The `edits_count` is the number of parsed edit operations.
///
/// The original and result lines are aligned with an LCS diff so that each
/// changed line is classified by *what actually happened*:
///
/// * a line present in the result but absent from the original is **inserted**;
/// * a line present in the original but absent from the result is **deleted**;
/// * a delete run immediately followed by an insert run is a **rewrite** of
///   that many lines (`SWAP`, `SWAP.BLK`), so those lines are paired up and
///   reported as **modified**.
///
/// A pure content-match heuristic cannot tell "insert a new line" from
/// "rewrite that line" — both look like one line replaced by one different
/// line — which made insert operations report the new line as modified and
/// an untouched neighbour as inserted.
fn compute_changeset(
    entries: &[crate::document::LineEntry],
    final_lines: &[String],
    edits_count: usize,
    _path: &Path,
    trailing_newline: bool,
    want_anchors: bool,
) -> ChangeSet {
    use crate::hash::{compute_file_hash, short_hash_value_indexed};

    let mut modified = Vec::new();
    let mut inserted = Vec::new();
    let mut deleted = Vec::new();

    // Filter entries to visible lines only (exclude trailing empty from
    // split('\n') on files with trailing newline) so lengths match
    // `final_lines` which comes from `split_normalized`.
    let visible_entries: Vec<&crate::document::LineEntry> = entries
        .iter()
        .enumerate()
        .filter(|(i, e)| !(e.content.is_empty() && *i == entries.len() - 1))
        .map(|(_, e)| e)
        .collect();

    let old_lines: Vec<&str> = visible_entries.iter().map(|e| e.content.as_str()).collect();
    let new_lines: Vec<&str> = final_lines.iter().map(|s| s.as_str()).collect();

    // A patch normally touches a small region, so the LCS table stays tiny.
    // The budget is generous enough to align a few thousand rewritten lines
    // exactly; past that `changeset_diff_ops` anchors on unique common lines
    // so sparse edits in huge files still align — only genuinely
    // unalignable chunks degrade to a wholesale delete+insert.
    let ops = changeset_diff_ops(&old_lines, &new_lines, MAX_LCS_CELLS);

    let mut orig_idx = 0usize; // 0-based cursor into old_lines
    let mut final_idx = 0usize; // 0-based cursor into final_lines
    // Original line numbers (1-indexed) removed by a delete run that has not
    // been classified yet. Held back so an immediately following insert run
    // can pair with them (a rewrite) rather than reporting a delete + insert.
    let mut pending_deletes: Vec<usize> = Vec::new();

    for op in ops {
        match op {
            crate::merge::DiffOp::Equal(count) => {
                // A delete run not followed by an insert run is a plain deletion.
                deleted.append(&mut pending_deletes);
                orig_idx += count;
                final_idx += count;
            }
            crate::merge::DiffOp::Delete(count) => {
                for _ in 0..count {
                    pending_deletes.push(orig_idx + 1);
                    orig_idx += 1;
                }
            }
            crate::merge::DiffOp::Insert(count) => {
                // The first lines of the insert run rewrite the pending
                // deletes; the surplus is a genuine insertion.
                let paired = pending_deletes.len().min(count);
                let _rewritten: Vec<usize> = pending_deletes.drain(..paired).collect();
                deleted.append(&mut pending_deletes);
                for offset in 0..count {
                    let line_num = final_idx + offset + 1;
                    let content = &new_lines[final_idx + offset];
                    let hash =
                        crate::hash::format_short_hash(short_hash_value_indexed(content, line_num));
                    let entry = ChangedLine {
                        line: line_num,
                        hash,
                        content: content.to_string(),
                    };
                    if offset < paired {
                        modified.push(entry);
                    } else {
                        inserted.push(entry);
                    }
                }
                final_idx += count;
            }
        }
    }
    // Trailing deletes with no insert run left to pair with.
    deleted.append(&mut pending_deletes);

    // Bound the reported change list. A genuinely wholesale rewrite (or an
    // unalignable fallback chunk) can produce tens of thousands of entries —
    // a token flood for agent consumers. Cap each category and say so; the
    // file itself is still written correctly, and `updated_anchors` (opt-in)
    // always carries the full listing.
    const MAX_REPORTED_CHANGES: usize = 1000;
    let mut omitted = 0usize;
    for vec in [&mut modified, &mut inserted] {
        if vec.len() > MAX_REPORTED_CHANGES {
            omitted += vec.len() - MAX_REPORTED_CHANGES;
            vec.truncate(MAX_REPORTED_CHANGES);
        }
    }
    if deleted.len() > MAX_REPORTED_CHANGES {
        omitted += deleted.len() - MAX_REPORTED_CHANGES;
        deleted.truncate(MAX_REPORTED_CHANGES);
    }
    if omitted > 0 {
        emit_apply_warning(&format!(
            "changeset truncated: {omitted} changed lines not shown \
             (showing first {MAX_REPORTED_CHANGES} per category)"
        ));
    }

    // The reported hash must match what `hashline read` computes for the
    // resulting file: the trailing newline is preserved only when the
    // original had one. The old code always appended '\n', so the reported
    // `path#hash` was stale for no-trailing-newline files.
    let final_text = if final_lines.is_empty() {
        String::new()
    } else if trailing_newline {
        final_lines.join("\n") + "\n"
    } else {
        final_lines.join("\n")
    };
    let file_hash = compute_file_hash(&final_text);

    // Full fresh anchor listing (issue #119): every result line with its new
    // number + new hash, computed with the same position-seeded hash the
    // `read` path uses so anchors are directly reusable. This clones and
    // hashes every result line, so it only runs when the caller opted in
    // (`--emit-anchors` / `return_updated_anchors` / env).
    let updated_anchors: Vec<ChangedLine> = if want_anchors {
        final_lines
            .iter()
            .enumerate()
            .map(|(i, content)| ChangedLine {
                line: i + 1,
                hash: crate::hash::format_short_hash(short_hash_value_indexed(content, i + 1)),
                content: content.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    ChangeSet {
        edits_applied: edits_count,
        modified,
        inserted,
        deleted,
        file_hash,
        line_count: final_lines.len(),
        updated_anchors,
    }
}

/// Line-level diff for the changeset. Like [`crate::merge::diff_ops_with_budget`]
/// but the over-budget middle is first anchored on lines that occur exactly
/// once on both sides (patience-style), so a sparse edit inside a huge file
/// still aligns exactly instead of degrading to one wholesale
/// delete+insert run — which the changeset renderer would pair line-by-line
/// into thousands of phantom "modified" entries (e.g. `DEL 1` + `DEL 9999`
/// on a 10k-line file reporting `changed=9999`).
fn changeset_diff_ops(old: &[&str], new: &[&str], max_cells: usize) -> Vec<crate::merge::DiffOp> {
    use crate::merge::DiffOp;

    // Common prefix/suffix, same trim diff_ops_with_budget applies.
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut o_end = old.len();
    let mut n_end = new.len();
    while o_end > prefix && n_end > prefix && old[o_end - 1] == new[n_end - 1] {
        o_end -= 1;
        n_end -= 1;
    }
    let mut ops = Vec::new();
    if prefix > 0 {
        ops.push(DiffOp::Equal(prefix));
    }
    anchored_diff(
        &old[prefix..o_end],
        &new[prefix..n_end],
        max_cells,
        0,
        &mut ops,
    );
    let suffix = old.len() - o_end;
    if suffix > 0 {
        ops.push(DiffOp::Equal(suffix));
    }
    ops
}

/// Recursively diff `old`/`new`. Small regions use the exact LCS; large
/// regions split on unique common anchors; regions with no anchors (or
/// absurd nesting depth) degrade to a wholesale delete+insert.
fn anchored_diff(
    old: &[&str],
    new: &[&str],
    max_cells: usize,
    depth: u32,
    ops: &mut Vec<crate::merge::DiffOp>,
) {
    use crate::merge::DiffOp;

    match (old.len(), new.len()) {
        (0, 0) => {}
        (_, 0) => ops.push(DiffOp::Delete(old.len())),
        (0, _) => ops.push(DiffOp::Insert(new.len())),
        (m, n) if m.saturating_mul(n) <= max_cells => {
            ops.extend(crate::merge::diff_ops_with_budget(old, new, max_cells));
        }
        _ if depth >= 32 => {
            ops.push(DiffOp::Delete(old.len()));
            ops.push(DiffOp::Insert(new.len()));
        }
        _ => {
            let anchors = unique_anchors(old, new);
            if anchors.is_empty() {
                ops.push(DiffOp::Delete(old.len()));
                ops.push(DiffOp::Insert(new.len()));
                return;
            }
            let (mut po, mut pn) = (0usize, 0usize);
            for (ao, an) in anchors {
                anchored_diff(&old[po..ao], &new[pn..an], max_cells, depth + 1, ops);
                ops.push(DiffOp::Equal(1));
                po = ao + 1;
                pn = an + 1;
            }
            anchored_diff(&old[po..], &new[pn..], max_cells, depth + 1, ops);
        }
    }
}

/// Pairs `(old_idx, new_idx)` of lines that occur exactly once in each
/// slice, kept in increasing order on both axes via a longest-increasing-
/// subsequence pass over the new-side positions.
fn unique_anchors(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    use std::collections::HashMap;

    let mut ocount: HashMap<&str, usize> = HashMap::with_capacity(old.len());
    for &l in old {
        *ocount.entry(l).or_insert(0) += 1;
    }
    let mut ncount: HashMap<&str, usize> = HashMap::with_capacity(new.len());
    for &l in new {
        *ncount.entry(l).or_insert(0) += 1;
    }
    // old position of each unique line.
    let mut opos: HashMap<&str, usize> = HashMap::with_capacity(old.len());
    for (i, &l) in old.iter().enumerate() {
        if ocount[l] == 1 {
            opos.insert(l, i);
        }
    }
    // Candidate matches, sorted by old position.
    let mut cand: Vec<(usize, usize)> = Vec::new();
    for (j, &l) in new.iter().enumerate() {
        if ncount[l] == 1 {
            if let Some(&oi) = opos.get(l) {
                cand.push((oi, j));
            }
        }
    }
    cand.sort_unstable();
    if cand.is_empty() {
        return cand;
    }
    // LIS over the new-side positions so anchors never cross.
    let seq: Vec<usize> = cand.iter().map(|&(_, nj)| nj).collect();
    let keep = lis_indices(&seq);
    keep.into_iter().map(|k| cand[k]).collect()
}

/// Indices of a longest strictly-increasing subsequence of `seq`
/// (patience sorting, O(n log n)).
fn lis_indices(seq: &[usize]) -> Vec<usize> {
    if seq.is_empty() {
        return Vec::new();
    }
    // tails[i] = index into seq of the smallest tail value for LIS length i+1.
    let mut tails: Vec<usize> = Vec::with_capacity(seq.len());
    let mut prev: Vec<usize> = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        // First tail >= v (lower_bound).
        let mut lo = 0usize;
        let mut hi = tails.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if seq[tails[mid]] < v {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo > 0 {
            prev[i] = tails[lo - 1];
        }
        if lo == tails.len() {
            tails.push(i);
        } else {
            tails[lo] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = *tails.last().unwrap();
    loop {
        out.push(cur);
        if prev[cur] == usize::MAX {
            break;
        }
        cur = prev[cur];
    }
    out.reverse();
    out
}

/// `apply_edits` plus a caller-owned per-patch clipboard. `CUT` ops write
/// captured lines into `clipboard` (keyed by register name); `PUT` ops read
/// them back out. A `PUT` referencing a register that was never captured is
/// a hard error. The clipboard lives only as long as this call.
pub fn apply_edits_with_clipboard(
    lines: &mut Vec<String>,
    entries: &[crate::document::LineEntry],
    path: &Path,
    edits: &[Edit],
    clipboard: &mut Clipboard,
) -> Result<(), HashlineError> {
    // entries may include a trailing empty line (from split('\n') on files
    // with trailing newlines) that is not displayed by `read`.  We use the
    // visible line count for anchor validation so that users cannot target
    // invisible lines (Bug #102).
    let visible_lines = {
        let n = entries.len();
        if n > 0 && entries[n - 1].content.is_empty() {
            n - 1
        } else {
            n
        }
    };
    let mut i = 0;

    // ---- gap-based position model -----------------------------------------
    //
    // The old implementation tracked a single net `shift[]` delta per
    // original line plus an `insert_count` side table. A scalar delta per
    // line cannot represent *interleaved* inserts: `INS.POST 50` + `DEL 50`
    // + `INS.POST 50` + `DEL 51` corrupted the table so `DEL 51` removed the
    // just-inserted payload instead of original line 51, and non-contiguous
    // or duplicate deletes silently mis-shifted every later op.
    //
    // The new model never indexes the live buffer by computed position.
    // The original document is `orig_len` lines separated by `orig_len + 1`
    // gaps: gap 0 precedes line 1, gap k sits between original lines k and
    // k+1, gap orig_len follows the last line. Every insert payload is
    // appended to its gap in patch order; deletes and SWAPs only ever mark
    // ORIGINAL lines as removed. Inserted text lives in gaps and can never
    // be re-anchored or deleted by a later op, so interleaved INS+DEL
    // sequences are structurally incapable of the old corruption.
    //
    // Final text = gap[0] ++ (line 1 if alive) ++ gap[1] ++ (line 2) ++ ...
    // ++ (line orig_len) ++ gap[orig_len].
    //
    // One refinement: a SWAP's replacement payload is NOT a gap insert — it
    // occupies the removed lines' content slot. Keeping it separate means
    // `INS.PRE N` always lands before a `SWAP N` replacement and `INS.POST N`
    // always after it, regardless of the order the ops appear in the patch.
    // `lines` is only overwritten on success — on any error it still holds
    // the original buffer, matching the pre-rewrite contract callers rely on.
    let mut orig: Vec<String> = lines.clone();
    let orig_len = orig.len();
    let n = entries.len();
    let mut deleted: Vec<bool> = vec![false; orig_len]; // 0-indexed by original line
    let mut gaps: Vec<Vec<String>> = vec![Vec::new(); orig_len + 1];
    // Replacement payload per original line (SWAP / SWAP.BLK). `deleted[i]`
    // is still true for these lines — the payload is emitted in the content
    // slot, not in a gap.
    let mut payload_at: Vec<Option<Vec<String>>> = vec![None; orig_len];

    /// Record an overlap rejection: a second op tried to remove an
    /// original line an earlier op already removed. Previously this was a
    /// silent skip whose shift accounting then corrupted later ops.
    fn already_deleted(line: usize) -> HashlineError {
        HashlineError::PatchRejected {
            reason: format!(
                "line {line} is already deleted by an earlier operation in this patch; \
                 each original line can be removed at most once"
            )
            .into_boxed_str(),
        }
    }

    while i < edits.len() {
        match &edits[i] {
            // ---- SWAP N..=M: -------------------------------------------------
            Edit::Insert {
                mode: Some(InsertMode::Replacement),
                cursor: Cursor::BeforeAnchor(start_anchor),
                expected_hash,
                ..
            } => {
                let anchor_line = start_anchor.line;
                if anchor_line > visible_lines {
                    return Err(HashlineError::InvalidAnchor {
                        anchor: format!(
                            "line {anchor_line} not found (file has {visible_lines} lines)",
                        ),
                    });
                }
                if let Some(expected) = expected_hash {
                    let anchor_index = anchor_line.wrapping_sub(1);
                    if anchor_index < entries.len() && *expected != entries[anchor_index].short_hash
                    {
                        return Err(HashlineError::StaleAnchor {
                            anchor: format!(
                                "{}:{}",
                                anchor_line,
                                crate::hash::format_short_hash(*expected)
                            )
                            .into(),
                            line: anchor_line,
                            expected: crate::hash::format_short_hash(*expected).into(),
                            actual: crate::hash::format_short_hash(
                                entries[anchor_index].short_hash,
                            )
                            .into(),
                            path: path.display().to_string().into(),
                            relocated_suffix: String::new().into(),
                        });
                    }
                }

                let mut replacement_texts: Vec<String> = Vec::new();
                let mut j = i;
                // Collect the `+` payload rows of this SWAP. All inserts of one
                // parsed op share its `line_num`, so that is the ownership key
                // used below (Bug #104).
                let op_line_num = match &edits[i] {
                    Edit::Insert { line_num, .. } => *line_num,
                    _ => unreachable!("match arm is Edit::Insert"),
                };
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Insert {
                            mode: Some(InsertMode::Replacement),
                            cursor: Cursor::BeforeAnchor(a),
                            text,
                            ..
                        } if a.line == anchor_line => {
                            replacement_texts.push(text.clone());
                            j += 1;
                        }
                        _ => break,
                    }
                }

                // Consume the `Delete` edits that belong to *this* SWAP (the
                // rows it removed). Only deletes emitted by the same parsed op
                // — same `line_num` — are owned by this SWAP. A `DEL` written
                // right after the SWAP's body is a separate operation and must
                // fall through to the `Edit::Delete` arm below, which resolves
                // its anchor correctly (Bug #104).
                let mut delete_lines: Vec<usize> = Vec::new();
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Delete {
                            anchor,
                            expected_hash,
                            line_num,
                            ..
                        } if *line_num == op_line_num => {
                            if let Some(expected) = expected_hash {
                                let anchor_index = anchor.line.wrapping_sub(1);
                                if anchor_index < entries.len()
                                    && *expected != entries[anchor_index].short_hash
                                {
                                    return Err(HashlineError::StaleAnchor {
                                        anchor: format!(
                                            "{}:{}",
                                            anchor.line,
                                            crate::hash::format_short_hash(*expected)
                                        )
                                        .into(),
                                        line: anchor.line,
                                        expected: crate::hash::format_short_hash(*expected).into(),
                                        actual: crate::hash::format_short_hash(
                                            entries[anchor_index].short_hash,
                                        )
                                        .into(),
                                        path: path.display().to_string().into(),
                                        relocated_suffix: String::new().into(),
                                    });
                                }
                            }
                            delete_lines.push(anchor.line);
                            j += 1;
                        }
                        _ => break,
                    }
                }

                let num_new = replacement_texts.len();
                let num_old = delete_lines.len();

                // Boundary echo detection + conservative repair (against the
                // original snapshot). Phase 7: exact-text boundary echoes are
                // auto-repaired (provable without a parser); delimiter-semantic
                // issues (dropped closers) still only warn — no parser, no repair.
                if num_old > 0 {
                    let start_l = anchor_line;
                    let end_l = anchor_line + num_old - 1;
                    if let Some((repaired, warning)) = crate::apply::repair_boundary_echo(
                        &replacement_texts,
                        start_l,
                        end_l,
                        entries,
                    ) {
                        emit_apply_warning(&warning);
                        replacement_texts = repaired;
                    } else {
                        let issues = crate::apply::detect_boundary_issues(
                            &replacement_texts,
                            start_l,
                            end_l,
                            entries,
                        );
                        for issue in &issues {
                            emit_apply_warning(issue);
                        }
                    }
                }

                // Overlap rejection: every line this SWAP removes must be an
                // original line no earlier op already removed. The old code
                // ignored `deleted[]` here and deleted whatever happened to
                // sit at the shift-adjusted live index — eating unrelated
                // lines. Reject instead of guessing.
                let mut sorted_dels = delete_lines.clone();
                sorted_dels.sort_unstable();
                for w in sorted_dels.windows(2) {
                    if w[0] == w[1] {
                        return Err(already_deleted(w[0]));
                    }
                }
                for &dl in &sorted_dels {
                    if dl < 1 || dl > orig_len {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: format!("line {dl} not found (file has {visible_lines} lines)",),
                        });
                    }
                    if deleted[dl - 1] {
                        return Err(already_deleted(dl));
                    }
                }
                for &dl in &sorted_dels {
                    deleted[dl - 1] = true;
                }

                // The replacement payload occupies the content slot of the
                // first removed original line — after the gap preceding it
                // (so `INS.PRE N` still lands before the replacement) and
                // before the gap following it (`INS.POST N` lands after).
                if num_new > 0 {
                    match sorted_dels.first() {
                        Some(&first) => payload_at[first - 1] = Some(replacement_texts),
                        None => gaps[anchor_line.saturating_sub(1)].extend(replacement_texts),
                    }
                }

                i = j;
            }

            // ---- DEL N / DEL N..=M -------------------------------------------
            Edit::Delete { .. } => {
                let mut del_lines: Vec<usize> = Vec::new();
                let mut j = i;
                while j < edits.len() {
                    match &edits[j] {
                        Edit::Delete {
                            anchor,
                            expected_hash,
                            ..
                        } => {
                            if let Some(expected) = expected_hash {
                                let anchor_index = anchor.line.wrapping_sub(1);
                                if anchor_index < entries.len()
                                    && *expected != entries[anchor_index].short_hash
                                {
                                    return Err(HashlineError::StaleAnchor {
                                        anchor: format!(
                                            "{}:{}",
                                            anchor.line,
                                            crate::hash::format_short_hash(*expected)
                                        )
                                        .into(),
                                        line: anchor.line,
                                        expected: crate::hash::format_short_hash(*expected).into(),
                                        actual: crate::hash::format_short_hash(
                                            entries[anchor_index].short_hash,
                                        )
                                        .into(),
                                        path: path.display().to_string().into(),
                                        relocated_suffix: String::new().into(),
                                    });
                                }
                            }
                            del_lines.push(anchor.line);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                // Validate all delete lines are in bounds
                for line in &del_lines {
                    if *line > visible_lines {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: format!(
                                "line {line} not found (file has {visible_lines} lines)",
                            ),
                        });
                    }
                }
                // Overlap rejection: duplicate or already-deleted targets are
                // a patch bug, not a no-op. The old code silently skipped the
                // live removal but still counted the line in its shift delta,
                // corrupting every later op in the patch.
                let mut sorted_dels = del_lines.clone();
                sorted_dels.sort_unstable();
                for w in sorted_dels.windows(2) {
                    if w[0] == w[1] {
                        return Err(already_deleted(w[0]));
                    }
                }
                for &dl in &sorted_dels {
                    if dl < 1 || dl > orig_len {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: format!("line {dl} not found (file has {visible_lines} lines)",),
                        });
                    }
                    if deleted[dl - 1] {
                        return Err(already_deleted(dl));
                    }
                    deleted[dl - 1] = true;
                }

                i = j;
            }

            // ---- INS.PRE / INS.POST / INS.HEAD / INS.TAIL --------------------
            Edit::Insert {
                cursor,
                text,
                expected_hash,
                ..
            } => {
                // Validate anchor bounds before processing (same check as
                // SWAP/DEL/CUT — anchor must reference an existing line).
                match &cursor {
                    Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a) if a.line > visible_lines => {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: format!(
                                "line {} not found (file has {visible_lines} lines)",
                                a.line,
                            ),
                        });
                    }
                    _ => {}
                }

                // Validate against the original snapshot, not the live buffer
                // which earlier operations in this batch may have changed.
                if let (Cursor::BeforeAnchor(a) | Cursor::AfterAnchor(a), Some(expected)) =
                    (cursor, expected_hash)
                {
                    let anchor_index = a.line.wrapping_sub(1);
                    if anchor_index < entries.len() && *expected != entries[anchor_index].short_hash
                    {
                        return Err(HashlineError::StaleAnchor {
                            anchor: format!(
                                "{}:{}",
                                a.line,
                                crate::hash::format_short_hash(*expected)
                            )
                            .into(),
                            line: a.line,
                            expected: crate::hash::format_short_hash(*expected).into(),
                            actual: crate::hash::format_short_hash(
                                entries[anchor_index].short_hash,
                            )
                            .into(),
                            path: path.display().to_string().into(),
                            relocated_suffix: String::new().into(),
                        });
                    }
                }

                // Anchors address the ORIGINAL snapshot. Inserts never touch
                // original lines — the payload is appended to the gap the
                // cursor names, in patch order. Because gap contents are
                // emitted strictly by gap index at rebuild time, an insert
                // can never be deleted or shifted by a later op (the old
                // `insert_count`/`shift` pair mis-ordered exactly this case).
                let gap_idx = match cursor {
                    // INS.PRE N → gap before original line N.
                    Cursor::BeforeAnchor(a) => a.line.saturating_sub(1),
                    // INS.POST N → gap after original line N.
                    Cursor::AfterAnchor(a) => a.line.min(orig_len),
                    Cursor::Bof => 0,
                    Cursor::Eof => orig_len,
                };
                gaps[gap_idx].push(text.clone());

                i += 1;
            }

            // ---- CUT N..=M @name -------------------------------------------
            Edit::Cut {
                anchor,
                end,
                register,
                expected_hash,
                ..
            } => {
                let start_line = anchor.line;
                let end_line = end.line;

                // Hash validation on the first anchor line, like DEL.
                if let Some(expected) = expected_hash {
                    let anchor_index = start_line.wrapping_sub(1);
                    if anchor_index < entries.len() && *expected != entries[anchor_index].short_hash
                    {
                        return Err(HashlineError::StaleAnchor {
                            anchor: format!(
                                "{}:{}",
                                start_line,
                                crate::hash::format_short_hash(*expected)
                            )
                            .into(),
                            line: start_line,
                            expected: crate::hash::format_short_hash(*expected).into(),
                            actual: crate::hash::format_short_hash(
                                entries[anchor_index].short_hash,
                            )
                            .into(),
                            path: path.display().to_string().into(),
                            relocated_suffix: String::new().into(),
                        });
                    }
                }
                // Validate the range is in bounds.
                for line in start_line..=end_line {
                    if line > visible_lines {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: format!(
                                "line {line} not found (file has {visible_lines} lines)",
                            ),
                        });
                    }
                }

                // Capture the ORIGINAL lines from the pre-edit snapshot.
                let mut captured: Vec<String> = Vec::new();
                for line in start_line..=end_line {
                    if line >= 1 && line <= n {
                        captured.push(entries[line - 1].content.clone());
                    }
                }
                if let Some(name) = register {
                    clipboard.named.insert(name.clone(), captured);
                } else {
                    clipboard.anon = Some(captured);
                }

                // Remove the range from the original-line set. Members an
                // earlier op already removed are skipped — CUT is a
                // capture-plus-delete, and the captured text always comes
                // from the original snapshot (see p9_cut_after_swap_*). This
                // differs from a bare DEL, which rejects overlaps outright.
                for dl in start_line..=end_line {
                    if dl >= 1 && dl <= orig_len && !deleted[dl - 1] {
                        deleted[dl - 1] = true;
                    }
                }

                i += 1;
            }

            // ---- PUT @name <N: ----------------------------------------------
            Edit::Paste {
                cursor, register, ..
            } => {
                let captured: Vec<String> = match register {
                    Some(name) => clipboard.named.get(name).cloned().ok_or_else(|| {
                        HashlineError::ClipboardMissingRegister {
                            register: name.clone(),
                        }
                    })?,
                    None => clipboard
                        .anon
                        .clone()
                        .ok_or(HashlineError::ClipboardEmptyAnon)?,
                };
                if captured.is_empty() {
                    i += 1;
                    continue;
                }

                // Resolve the gap the captured lines paste into. PUT always
                // targets "before line N" (gap N-1) or the file head (gap 0);
                // like INS, the payload joins the gap's patch-ordered list.
                let gap_idx = match cursor {
                    Cursor::BeforeAnchor(a) => {
                        if a.line > visible_lines {
                            return Err(HashlineError::InvalidAnchor {
                                anchor: format!(
                                    "line {} not found (file has {visible_lines} lines)",
                                    a.line,
                                ),
                            });
                        }
                        a.line.saturating_sub(1)
                    }
                    Cursor::Bof => 0,
                    _ => {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: "PUT target must be before a line".to_string(),
                        });
                    }
                };
                gaps[gap_idx].extend(captured);

                i += 1;
            }

            // ---- SWAP.BLK N: / DEL.BLK N / INS.BLK.POST N: -----------------
            Edit::Block {
                anchor,
                payloads,
                mode,
                expected_hash,
                ..
            } => {
                let line_no = anchor.line;

                if line_no > visible_lines {
                    return Err(HashlineError::InvalidAnchor {
                        anchor: format!(
                            "line {line_no} not found (file has {visible_lines} lines)",
                        ),
                    });
                }
                let anchor_index = line_no.wrapping_sub(1);

                // Hash validation for block ops — thread optional hash through
                // (Bug #89-2: validate block-anchor hash just like line ops)
                if let Some(expected) = expected_hash {
                    let anchor_idx = line_no.wrapping_sub(1);
                    if anchor_idx < entries.len() && *expected != entries[anchor_idx].short_hash {
                        return Err(HashlineError::StaleAnchor {
                            anchor: format!(
                                "{}:{}",
                                line_no,
                                crate::hash::format_short_hash(*expected)
                            )
                            .into(),
                            line: line_no,
                            expected: crate::hash::format_short_hash(*expected).into(),
                            actual: crate::hash::format_short_hash(entries[anchor_idx].short_hash)
                                .into(),
                            path: path.display().to_string().into(),
                            relocated_suffix: String::new().into(),
                        });
                    }
                }

                // Resolve the syntactic block containing the anchor via the
                // canonical resolver (builtin_resolver) — the same engine the
                // Editor and find-block use, so all three paths agree.
                //
                // NOTE: this resolves against the ORIGINAL snapshot. After
                // prior edits in the same patch remove/insert lines, the
                // original line numbers may no longer correspond to the
                // intended blocks. The gap model below makes the *position*
                // translation exact, but the span itself is still resolved
                // on the pre-edit text — a known limitation. To stay
                // fail-closed, a block op whose resolved span overlaps lines
                // an earlier op already removed is rejected outright rather
                // than silently retargeting a neighbouring block.
                let (orig_block_start, orig_block_end) =
                    crate::builtin_resolver::resolve_span(entries, anchor_index, path)
                        .ok_or(HashlineError::UnbalancedBlock { line_no })?;

                // Convert the 0-based inclusive span over `entries` to 1-based
                // original line numbers, clamped to real lines (`orig_len`).
                // `entries` may carry a phantom trailing empty entry for the
                // final newline — it is not a deletable line.
                let start_l = orig_block_start + 1;
                let end_l = (orig_block_end + 1).min(orig_len);
                if start_l > end_l || start_l > orig_len {
                    return Err(HashlineError::UnbalancedBlock { line_no });
                }

                // Overlap rejection: any already-removed line inside the
                // resolved span means the block no longer exists as written.
                for dl in start_l..=end_l {
                    if deleted[dl - 1] {
                        return Err(HashlineError::PatchRejected {
                            reason: format!(
                                "block op at line {line_no} overlaps line {dl}, already \
                                 removed by an earlier operation in this patch"
                            )
                            .into_boxed_str(),
                        });
                    }
                }

                // SWAP.BLK gets the same boundary-echo / dropped-closer
                // checks a line-range SWAP does: the payload replaces the
                // whole resolved span, so an echo of the lines just outside
                // the span (or a dropped structural closer) is just as much
                // of an LLM mistake here as on `SWAP N..M:`.
                let mut effective_payloads: Option<Vec<String>> = None;
                if mode.is_none() && !payloads.is_empty() {
                    let mut p = payloads.clone();
                    if let Some((repaired, warning)) =
                        crate::apply::repair_boundary_echo(&p, start_l, end_l, entries)
                    {
                        emit_apply_warning(&warning);
                        p = repaired;
                    } else {
                        for issue in
                            crate::apply::detect_boundary_issues(&p, start_l, end_l, entries)
                        {
                            emit_apply_warning(&issue);
                        }
                    }
                    effective_payloads = Some(p);
                }

                match mode {
                    None if payloads.is_empty() => {
                        // DEL.BLK N (and empty-payload SWAP.BLK) — mark the
                        // whole span deleted.
                        for dl in start_l..=end_l {
                            deleted[dl - 1] = true;
                        }
                        // Consume the trailing blank line after the block —
                        // but only when it is a real, still-alive ORIGINAL
                        // line that is blank in the snapshot. The old code
                        // inspected the live buffer and neither marked the
                        // consumed line deleted[] nor distinguished an
                        // inserted blank from an original one.
                        let blank_l = end_l + 1;
                        if blank_l <= orig_len
                            && !deleted[blank_l - 1]
                            && entries
                                .get(blank_l - 1)
                                .map(|e| e.content.trim().is_empty())
                                .unwrap_or(false)
                        {
                            deleted[blank_l - 1] = true;
                        }
                    }
                    None => {
                        // SWAP.BLK N: replace the entire block with payload.
                        // The payload occupies the content slot of the
                        // block's first line — same slot a line-range SWAP
                        // uses.
                        for dl in start_l..=end_l {
                            deleted[dl - 1] = true;
                        }
                        payload_at[start_l - 1] =
                            Some(effective_payloads.unwrap_or_else(|| payloads.clone()));
                    }
                    Some(BlockMode::InsertAfter) => {
                        // INS.BLK.POST N: insert after the last line of the
                        // block → the gap following original line `end_l`.
                        gaps[end_l.min(orig_len)].extend(payloads.iter().cloned());
                    }
                    Some(BlockMode::InsertBefore) => {
                        // INS.BLK.PRE N: insert before the first line of the
                        // block → the gap preceding original line `start_l`.
                        gaps[start_l - 1].extend(payloads.iter().cloned());
                    }
                }

                i += 1;
            }
        }
    }

    // ---- rebuild the live buffer ------------------------------------------
    // gap[0], then per original line: content slot (original text, SWAP
    // payload, or nothing when deleted) followed by that line's trailing gap.
    let mut out: Vec<String> = std::mem::take(&mut gaps[0]);
    for idx in 0..orig_len {
        if !deleted[idx] {
            out.push(std::mem::take(&mut orig[idx]));
        } else if let Some(payload) = payload_at[idx].take() {
            out.extend(payload);
        }
        out.append(&mut gaps[idx + 1]);
    }
    *lines = out;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_text_ext(original: &str, patch_text: &str, ext: &str) -> String {
        let (edits, _warnings, _file_op, _aborted) = parse_patch(patch_text);
        let mut lines: Vec<String> = if original.is_empty() {
            Vec::new()
        } else {
            original.split('\n').map(|s| s.to_string()).collect()
        };
        let path_str = format!("test.{ext}");
        let entries_with_content: Vec<crate::document::LineEntry> = lines
            .iter()
            .map(|s| crate::document::LineEntry {
                content: s.clone(),
                short_hash: crate::hash::short_hash_value(s),
            })
            .collect();
        apply_edits(
            &mut lines,
            &entries_with_content,
            std::path::Path::new(&path_str),
            &edits,
        )
        .expect("edit should succeed");
        lines.join("\n")
    }

    fn apply_text(original: &str, patch_text: &str) -> String {
        apply_text_ext(original, patch_text, "rs")
    }

    #[test]
    fn insert_anchors_validate_original_snapshot_hashes() {
        let original = "alpha\nbeta\ngamma\n";
        let entries: Vec<crate::document::LineEntry> = split_normalized(original)
            .into_iter()
            .enumerate()
            .map(|(i, content)| crate::document::LineEntry {
                short_hash: crate::hash::short_hash_value_indexed(&content, i + 1),
                content,
            })
            .collect();
        let hash = entries[1].short_hash;
        let valid_hash = crate::hash::format_short_hash(hash);
        let stale_hash = crate::hash::format_short_hash(hash ^ 1);
        for (op, expected) in [
            ("INS.PRE", "alpha\nadded\nbeta\ngamma"),
            ("INS.POST", "alpha\nbeta\nadded\ngamma"),
        ] {
            for address in [format!("2:{valid_hash}"), "2".to_string()] {
                let (edits, _, _, aborted) = parse_patch(&format!("{op} {address}:\n+added"));
                assert!(!aborted);
                let mut lines = split_normalized(original);
                apply_edits(&mut lines, &entries, Path::new("test.txt"), &edits).unwrap();
                assert_eq!(lines.join("\n"), expected);
            }
            // Earlier operations change and shift the anchor's live content.
            // Its hash must still be checked against the original entries.
            let prefix = "INS.HEAD:\n+header\nSWAP 2:\n+changed";
            let (edits, _, _, _) = parse_patch(&format!("{prefix}\n{op} 2:{valid_hash}:\n+added"));
            let mut lines = split_normalized(original);
            apply_edits(&mut lines, &entries, Path::new("test.txt"), &edits).unwrap();
            assert_eq!(
                lines.join("\n"),
                expected
                    .replacen("alpha", "header\nalpha", 1)
                    .replace("beta", "changed")
            );

            let mut lines = split_normalized(original);
            let (edits, _, _, _) = parse_patch(&format!("{op} 2:{stale_hash}:\n+added"));
            let error =
                apply_edits(&mut lines, &entries, Path::new("test.txt"), &edits).unwrap_err();
            assert!(matches!(error, HashlineError::StaleAnchor { line: 2, .. }));
            let (swap, _, _, _) = parse_patch(&format!("SWAP 2:{stale_hash}:\n+added"));
            let swap_error =
                apply_edits(&mut lines, &entries, Path::new("test.txt"), &swap).unwrap_err();
            assert_eq!(error.to_string(), swap_error.to_string());
            assert_eq!(lines, split_normalized(original));
        }
    }

    #[test]
    fn test_swap_single_line() {
        let result = apply_text("line1\nline2\nline3", "SWAP 2:\n+replaced2");
        assert_eq!(result, "line1\nreplaced2\nline3");
    }

    #[test]
    fn test_swap_range() {
        let result = apply_text("line1\nline2\nline3\nline4", "SWAP 2..3:\n+x\n+y");
        assert_eq!(result, "line1\nx\ny\nline4");
    }

    #[test]
    fn test_delete_single() {
        let result = apply_text("line1\nline2\nline3", "DEL 2");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_delete_range() {
        let result = apply_text("line1\nline2\nline3\nline4", "DEL 2..3");
        assert_eq!(result, "line1\nline4");
    }

    #[test]
    fn test_insert_post() {
        let result = apply_text("line1\nline2", "INS.POST 1:\n+inserted");
        assert_eq!(result, "line1\ninserted\nline2");
    }

    #[test]
    fn test_insert_pre() {
        let result = apply_text("line1\nline2", "INS.PRE 2:\n+inserted");
        assert_eq!(result, "line1\ninserted\nline2");
    }

    #[test]
    fn test_insert_head() {
        let result = apply_text("line1\nline2", "INS.HEAD:\n+header");
        assert_eq!(result, "header\nline1\nline2");
    }

    #[test]
    fn test_insert_tail() {
        let result = apply_text("line1\nline2", "INS.TAIL:\n+footer");
        assert_eq!(result, "line1\nline2\nfooter");
    }

    #[test]
    fn test_multiple_insert_post() {
        let result = apply_text("line1\nline2", "INS.POST 1:\n+a\n+b");
        assert_eq!(result, "line1\na\nb\nline2");
    }

    #[test]
    fn test_swap_without_body_reduces_to_delete_range() {
        let result = apply_text("line1\nline2\nline3", "SWAP 2:");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_empty_original_swapped() {
        let result = apply_text("", "INS.HEAD:\n+newline");
        assert_eq!(result, "newline");
    }

    #[test]
    fn test_patch_with_header() {
        let result = apply_text("line1\nline2\nline3", "[file.txt#abcd]\nDEL 2");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_swap_then_insert() {
        let result = apply_text("a\nb\nc", "SWAP 2:\n+x\nINS.TAIL:\n+y");
        assert_eq!(result, "a\nx\nc\ny");
    }

    // ---- Block operation tests ----

    #[test]
    fn test_swap_block_rust_function() {
        // Block: fn hello() { ... } spans lines 1..6 (0-indexed 0..5)
        let original =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let patch = "SWAP.BLK 1:\n+fn replaced() {\n+    // new body\n+}\n";
        let result = apply_text(original, patch);
        // The old block (6 lines) is replaced with 3 replacement lines
        assert_eq!(result, "fn replaced() {\n    // new body\n}\n");
    }

    #[test]
    fn test_swap_block_inner() {
        // Anchor at line 3 (if true { ... }) should replace the if-block, not the outer fn
        let original =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let patch = "SWAP.BLK 3:\n+if false {\n+        // nothing\n+    }\n";
        let result = apply_text(original, patch);
        assert_eq!(
            result,
            "fn hello() {\n    let x = 1;\nif false {\n        // nothing\n    }\n}\n"
        );
    }

    #[test]
    fn test_delete_block_rust() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "DEL.BLK 1";
        let result = apply_text(original, patch);
        assert_eq!(result, "");
    }

    #[test]
    fn test_insert_after_block_rust() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "INS.BLK.POST 1:\n+fn world() {\n+    let y = 2;\n+}\n";
        let result = apply_text(original, patch);
        assert_eq!(
            result,
            "fn hello() {\n    let x = 1;\n}\nfn world() {\n    let y = 2;\n}\n"
        );
    }

    #[test]
    fn test_swap_block_python() {
        let original = "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        let result = apply_text_ext(original, "SWAP.BLK 1:\n+def hi():\n+    pass\n", "py");
        assert_eq!(result, "def hi():\n    pass\n");
    }

    // ---- Empty-patch detection (fixes #58) ----

    /// Build a synthetic `Edit::Insert` for empty-patch assertions.
    fn parse_only(text: &str) -> Vec<crate::types::Edit> {
        let (edits, _warnings, _file_op, _aborted) = parse_patch(text);
        edits
    }

    #[test]
    fn parse_patch_empty_string_yields_no_edits() {
        let edits = parse_only("");
        assert!(edits.is_empty(), "expected zero edits for empty patch");
    }

    #[test]
    fn parse_patch_unparseable_garbage_yields_no_edits() {
        let edits = parse_only("this is not a hashline patch\nborked\n!!");
        assert!(edits.is_empty(), "expected zero edits for garbage patch");
    }

    #[test]
    fn parse_patch_hash_suffix_yields_real_swap() {
        // From issue #56: `SWAP 2:67:` used to be silently rejected by the
        // tokenizer, producing zero edits. After the fix, the hash suffix
        // is consumed and the SWAP produces a replacement insert + delete.
        let edits = parse_only("SWAP 2:67:\n+REPLACED");
        assert_eq!(
            edits.len(),
            2,
            "expected 1 insert + 1 delete, got {edits:?}"
        );
        match &edits[0] {
            crate::types::Edit::Insert {
                cursor: crate::types::Cursor::BeforeAnchor(a),
                text,
                mode,
                ..
            } => {
                assert_eq!(a.line, 2);
                assert_eq!(text, "REPLACED");
                assert!(matches!(mode, Some(crate::types::InsertMode::Replacement)));
            }
            other => panic!("expected BeforeAnchor insert, got {other:?}"),
        }
        match &edits[1] {
            crate::types::Edit::Delete {
                anchor,
                expected_hash,
                ..
            } => {
                assert_eq!(anchor.line, 2);
                assert_eq!(*expected_hash, Some(0x67));
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn test_escape_plus_in_payload() {
        // `++x` should produce content `+x`
        let result = apply_text("before\nafter", "SWAP 2:\n++x");
        assert_eq!(result, "before\n+x");
    }

    #[test]
    fn test_escape_dash_in_payload() {
        // `+-x` should produce content `-x`
        let result = apply_text("before\nafter", "SWAP 2:\n+-x");
        assert_eq!(result, "before\n-x");
    }

    #[test]
    fn test_minus_row_rejected_warning() {
        // A bare `-something` line should emit MINUS_ROW_REJECTED warning
        let (_edits, warnings, _file_op, _aborted) =
            crate::parser::parse_patch("SWAP 2:\n+-ok\n-bad\n++also_ok");
        // The `-bad` line should generate a warning at least once
        let has_minus_warning = warnings
            .iter()
            .any(|w| w.contains("`-` rows accepted as content"));
        assert!(
            has_minus_warning,
            "expected MINUS_ROW_REJECTED warning, got warnings: {warnings:?}"
        );
    }

    #[test]
    fn parse_patch_hash_suffix_on_range() {
        // Range form: `SWAP 2..3:67:` should also accept the hash suffix
        // and produce 2 inserts + 2 deletes.
        let edits = parse_only("SWAP 2..3:67:\n+AAA\n+BBB");
        assert_eq!(
            edits.len(),
            4,
            "expected 2 inserts + 2 deletes, got {edits:?}"
        );
    }

    // =====================================================================
    // Regression tests for Issue #89
    // =====================================================================

    /// Apply edits and return Result (for error-path testing).
    fn apply_text_result(
        original: &str,
        patch_text: &str,
        ext: &str,
    ) -> Result<String, HashlineError> {
        let (edits, _warnings, _file_op, _aborted) = parse_patch(patch_text);
        let mut lines: Vec<String> = split_normalized(original);
        let path_str = format!("test.{ext}");
        let entries_with_content: Vec<crate::document::LineEntry> = lines
            .iter()
            .map(|s| crate::document::LineEntry {
                content: s.clone(),
                short_hash: crate::hash::short_hash_value(s),
            })
            .collect();
        apply_edits(
            &mut lines,
            &entries_with_content,
            std::path::Path::new(&path_str),
            &edits,
        )?;
        let result = lines.join("\n");
        Ok(if original.ends_with('\n') && !lines.is_empty() {
            result + "\n"
        } else {
            result
        })
    }

    // ---- Bug #89-1: multi-op hash anchor validation vs live buffer ----

    #[test]
    fn bug89_multi_op_insert_then_hash_swap_respects_shift() {
        // INS.POST 1: inserts a line after line 1, shifting everything down.
        // SWAP 4 should then target original line 4 (which now lives at index 3
        // in the shifted buffer), not CCC at original line 3.
        let original = "AAA\nBBB\nCCC\nDDD";
        let patch = "INS.POST 1:\n+XXX\nSWAP 4:\n+ZZZ";
        let result = apply_text_result(original, patch, "txt").unwrap();
        // After INS.POST 1: shifts everything down, SWAP 4 correctly targets
        // original line 4 (DDD), not CCC at original line 3.
        assert_eq!(result, "AAA\nXXX\nBBB\nCCC\nZZZ");
    }

    #[test]
    fn bug89_multi_op_hash_rejected_on_wrong_line_after_insert() {
        // Same as above but with explicit hash on SWAP. The hash `ac` is DDD's.
        // After the insert shifts everything, the correct original line 4 (DDD)
        // still carries `ac`, but if the applier uses the live position it would
        // match CCC instead.  Use the test-only helper that constructs entries
        // from the original text so hashes are real.
        let original = "AAA\nBBB\nCCC\nDDD";
        let patch = "INS.POST 1:\n+XXX\nSWAP 4:ac:\n+ZZZ";
        // Should succeed — original line 4 (DDD, hash `ac`) is the target.
        let result = apply_text_result(original, patch, "txt").unwrap();
        assert_eq!(result, "AAA\nXXX\nBBB\nCCC\nZZZ");
    }

    // ---- Bug #89-2: .BLK ops should validate hash anchors ----

    #[test]
    fn bug89_blk_swap_with_bad_hash_errors() {
        // SWAP.BLK N:ff: on a line whose actual hash is not `ff` should
        // produce a StaleAnchor error, just like a plain line op.
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "SWAP.BLK 1:ff:\n+fn replaced() {\n+}\n";
        let err = apply_text_result(original, patch, "rs").unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error, got: {err}"
        );
    }

    #[test]
    fn bug89_blk_del_with_bad_hash_errors() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "DEL.BLK 1:ff";
        let err = apply_text_result(original, patch, "rs").unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error, got: {err}"
        );
    }

    #[test]
    fn bug89_blk_ins_after_with_valid_hash_succeeds() {
        // Use a hash that actually matches line 1's short hash.
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let h = crate::hash::format_short_hash(crate::hash::short_hash_value("fn hello() {"));
        let patch = format!("INS.BLK.POST 1:{h}:\n+fn world() {{\n+}}\n");
        let result = apply_text_result(original, &patch, "rs").unwrap();
        assert!(result.contains("fn world()"));
    }

    // ---- Bug #89-3: out-of-range / unresolved anchor ----

    #[test]
    fn bug89_blk_out_of_range_errors() {
        // SWAP.BLK 999 on a 3-line file must error, not report success.
        let original = "a\nb\nc\n";
        let patch = "SWAP.BLK 999:\n+x\n";
        let err = apply_text_result(original, patch, "rs").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected InvalidAnchor error, got: {err}"
        );
    }

    // ---- Bug #89-4: Python block over-detection ----

    #[test]
    fn bug89_python_block_at_interior_line_only_replaces_that_line() {
        // Anchoring SWAP.BLK at an interior line (e.g. a comment) should
        // replace only the smallest enclosing block (the line itself or its
        // indentation group), not the whole function.
        let original = "def hello():\n    # comment\n    x = 1\n    return x";
        let patch = "SWAP.BLK 2:\n+    # replaced comment\n";
        let result = apply_text_result(original, patch, "py").unwrap();
        // Only line 2 should be replaced; the rest of the function stays.
        assert_eq!(
            result,
            "def hello():\n    # replaced comment\n    x = 1\n    return x"
        );
    }

    // ---- Bug #89-5: INS.BLK alias and INS.BLK.PRE ----

    #[test]
    fn bug89_ins_blk_alias_works() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "INS.BLK 1:\n+fn world() {\n+}\n";
        let result = apply_text_result(original, patch, "rs").unwrap();
        assert_eq!(result, "fn hello() {\n    let x = 1;\n}\nfn world() {\n}\n");
    }

    #[test]
    fn bug89_ins_blk_pre_works() {
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "INS.BLK.PRE 1:\n+fn preamble() {\n+}\n";
        let result = apply_text_result(original, patch, "rs").unwrap();
        assert_eq!(
            result,
            "fn preamble() {\n}\nfn hello() {\n    let x = 1;\n}\n"
        );
    }

    // ---- bd-blk-op-semantics-90b: block-op syntax strictness --------------

    #[test]
    fn blk_range_suffix_rejected_not_silently_ignored() {
        // `..M` on a block op is meaningless — the resolver expands the
        // anchor to a syntactic span. It must be rejected, not dropped.
        for header in [
            "SWAP.BLK 1..2:",
            "DEL.BLK 1..2",
            "INS.BLK.POST 1..2:",
            "INS.BLK.PRE 1..2:",
            "INS.BLK 1..2:",
        ] {
            let (_edits, _w, _f, aborted) = crate::parser::parse_patch(&format!("{header}\n+x\n"));
            assert!(aborted, "{header} should abort the patch");
        }
    }

    #[test]
    fn del_blk_does_not_swallow_op_like_garbage() {
        // A malformed op line after DEL.BLK must hit the unknown-op guard
        // instead of being silently consumed as payload.
        let (_edits, warnings, _f, aborted) =
            crate::parser::parse_patch("DEL.BLK 1\nSWAP.BLK 2..3:\n+x\n");
        assert!(aborted, "expected patch abort, got warnings {warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("SWAP.BLK")
                || w.contains("unknown operation")
                || w.contains("malformed")),
            "expected an op-candidate warning, got {warnings:?}"
        );
    }

    #[test]
    fn swap_blk_empty_payload_deletes_block() {
        // Same convention as `SWAP N:` with an empty body: delete the span.
        let original = "fn a() {\n    x\n}\nfn b() {\n    y\n}\n";
        let result = apply_text(original, "SWAP.BLK 1:");
        assert_eq!(result, "fn b() {\n    y\n}\n");
    }

    #[test]
    fn ins_blk_empty_payload_warns() {
        let (_edits, warnings, _f, _aborted) =
            crate::parser::parse_patch("INS.BLK.POST 1:\nSWAP 2:\n+x\n");
        assert!(
            warnings.iter().any(|w| w.contains("no payload")),
            "expected empty-payload warning, got {warnings:?}"
        );
    }

    #[test]
    fn blk_malformed_hash_rejected() {
        for header in ["SWAP.BLK 1:zz:", "DEL.BLK 1:zz", "INS.BLK.POST 1:zz:"] {
            let (_edits, _w, _f, aborted) = crate::parser::parse_patch(&format!("{header}\n+x\n"));
            assert!(aborted, "{header} should abort the patch");
        }
    }

    #[test]
    fn del_blk_python_body_anchor_deletes_enclosing_def() {
        let original = "def f():\n    x = 1\n    return x\n\ndef g():\n    pass\n";
        let result = apply_text_ext(original, "DEL.BLK 2", "py");
        assert_eq!(result, "def g():\n    pass\n");
    }

    #[test]
    fn bug89_case_insensitive_keywords() {
        // `swap`, `del`, `ins.post` etc should work case-insensitively.
        let original = "AAA\nBBB\nCCC";
        let patch = "swap 2:\n+replaced\n";
        let result = apply_text_result(original, patch, "txt").unwrap();
        assert_eq!(result, "AAA\nreplaced\nCCC");
    }

    #[test]
    fn bug89_blk_hash_on_swap_block_mismatch() {
        // Specific example from the issue: SWAP.BLK 4:ff: should error
        // when line 4's actual hash is not ff.
        let original = "AAA\nBBB\nCCC\nDDD\n";
        let patch = "SWAP.BLK 4:ff:\n+ZZZ\n";
        let err = apply_text_result(original, patch, "txt").unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error for BLK hash mismatch, got: {err}"
        );
    }

    #[test]
    fn bug89_blk_valid_hash_swap_block_succeeds() {
        // Use RS extension so brace-matching is used.
        let original = "fn a() {}\nfn b() {\n    let x = 1;\n}";
        // Line 1: `fn a() {}` is a single-line brace block
        let h = crate::hash::format_short_hash(crate::hash::short_hash_value("fn a() {}"));
        let patch = format!("SWAP.BLK 1:{h}:\n+fn replaced() {{\n}}\n");
        let result = apply_text_result(original, &patch, "rs").unwrap();
        assert_eq!(result, "fn replaced() {\n}\nfn b() {\n    let x = 1;\n}");
    }

    // =====================================================================
    // Regression tests for Issue #95 — SWAP.BLK / DEL.BLK range format
    // =====================================================================

    #[test]
    fn bug95_swap_blk_range_replaces_concrete_lines() {
        // SWAP.BLK N M with two anchors must be treated as a concrete range,
        // not a block to resolve.
        let result = apply_text(
            "line1\nline2\nline3\nline4\nline5",
            "SWAP.BLK 2 4:\n+x\n+y\n+z",
        );
        assert_eq!(result, "line1\nx\ny\nz\nline5");
    }

    #[test]
    fn bug95_swap_blk_range_with_hash() {
        // SWAP.BLK N:HH M:HH with hashes on both anchors
        let original = "line1\nline2\nline3\nline4\nline5";
        let h2 = crate::hash::format_short_hash(crate::hash::short_hash_value("line2"));
        let h4 = crate::hash::format_short_hash(crate::hash::short_hash_value("line4"));
        let patch = format!("SWAP.BLK 2:{h2} 4:{h4}:\n+x\n+y\n+z");
        let result = apply_text(original, &patch);
        assert_eq!(result, "line1\nx\ny\nz\nline5");
    }

    #[test]
    fn bug95_del_blk_range_deletes_concrete_lines() {
        // DEL.BLK N M with two anchors must delete the concrete range
        let result = apply_text("line1\nline2\nline3\nline4\nline5", "DEL.BLK 2 4");
        assert_eq!(result, "line1\nline5");
    }

    #[test]
    fn bug95_swap_blk_range_after_swap_no_corruption() {
        // SWAP then SWAP.BLK range in sequence — the corruption scenario
        let result = apply_text(
            "line1\nline2\nline3\nline4\nline5",
            "SWAP 1:\n+updated1\nSWAP.BLK 3 4:\n+x\n+y",
        );
        assert_eq!(result, "updated1\nline2\nx\ny\nline5");
    }

    #[test]
    fn bug95_swap_blk_single_anchor_still_resolves_block() {
        // SWAP.BLK N: single-anchor format must still resolve via built-in resolver
        let original =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let patch = "SWAP.BLK 1:\n+fn replaced() {\n+    // new body\n+}\n";
        let result = apply_text(original, patch);
        // The old block (6 lines) is replaced with 3 replacement lines
        assert_eq!(result, "fn replaced() {\n    // new body\n}\n");
    }

    #[test]
    fn bug95_del_blk_single_anchor_still_resolves_block() {
        // DEL.BLK N single-anchor format must still resolve via built-in resolver
        let original = "fn hello() {\n    let x = 1;\n}\n";
        let patch = "DEL.BLK 1";
        let result = apply_text(original, patch);
        assert_eq!(result, "");
    }

    #[test]
    fn bug95_swap_blk_range_followed_by_insert() {
        // SWAP.BLK range then INS in the same patch
        let result = apply_text(
            "line1\nline2\nline3\nline4\nline5",
            "SWAP.BLK 2 3:\n+x\n+y\nINS.TAIL:\n+z",
        );
        assert_eq!(result, "line1\nx\ny\nline4\nline5\nz");
    }

    #[test]
    fn bug95_del_blk_range_followed_by_insert() {
        // DEL.BLK range then INS in the same patch
        let result = apply_text(
            "line1\nline2\nline3\nline4\nline5",
            "DEL.BLK 2 4\nINS.HEAD:\n+prefix",
        );
        assert_eq!(result, "prefix\nline1\nline5");
    }

    #[test]
    fn bug95_swap_blk_range_no_hash_on_second_anchor() {
        // First anchor has hash, second doesn't
        let original = "line1\nline2\nline3\nline4\nline5";
        let h2 = crate::hash::format_short_hash(crate::hash::short_hash_value("line2"));
        let patch = format!("SWAP.BLK 2:{h2} 4:\n+x\n+y\n+z");
        let result = apply_text(original, &patch);
        assert_eq!(result, "line1\nx\ny\nz\nline5");
    }

    // =====================================================================
    // Regression tests for Issue #93 — content-loss bugs
    // =====================================================================

    #[test]
    fn bug93_interior_blank_lines_in_ins_post() {
        // Problem A: blank lines inside an inserted block must survive
        let result = apply_text(
            "Intro line.\nAnchor paragraph.\nTrailing line.",
            "INS.POST 2:\n+First paragraph.\n\n+Second paragraph.",
        );
        assert_eq!(
            result,
            "Intro line.\nAnchor paragraph.\nFirst paragraph.\n\nSecond paragraph.\nTrailing line."
        );
    }

    #[test]
    fn bug93_bare_minus_lines_preserved() {
        // Problem B: bare `-` lines must be preserved as content (with warning)
        let result = apply_text(
            "Intro line.\nAnchor paragraph.\nTrailing line.",
            "INS.POST 2:\n+Here is a list:\n- first item\n- second item\n+Done.",
        );
        assert_eq!(
            result,
            "Intro line.\nAnchor paragraph.\nHere is a list:\n- first item\n- second item\nDone.\nTrailing line."
        );
    }

    #[test]
    fn bug93_trailing_blanks_still_stripped() {
        // Trailing blank lines at the end of a payload block should still be dropped
        let result = apply_text("AAA\nBBB\nCCC", "INS.POST 2:\n+XXX\n+YYY\n");
        assert_eq!(result, "AAA\nBBB\nXXX\nYYY\nCCC");
    }

    #[test]
    fn bug93_interior_and_trailing_blanks() {
        // Interior blank preserved, trailing blanks dropped
        let result = apply_text("AAA\nBBB\nCCC", "INS.POST 2:\n+XXX\n\n+YYY\n");
        assert_eq!(result, "AAA\nBBB\nXXX\n\nYYY\nCCC");
    }

    // =====================================================================
    // Regression tests for Issue #104 — DEL after a bodied SWAP must not be
    // swallowed into the SWAP's removal range (the delete's count was added
    // to the SWAP while its anchor was discarded, silently deleting the
    // wrong lines).
    // =====================================================================

    const L12: &str = "L01\nL02\nL03\nL04\nL05\nL06\nL07\nL08\nL09\nL10\nL11\nL12";

    #[test]
    fn bug104_del_after_swap_non_adjacent_targets() {
        // Case 1: DEL 5 after SWAP 10 — L05 removed, L10→S10, L11 intact.
        let result = apply_text(L12, "SWAP 10:\n+S10\nDEL 5");
        assert_eq!(
            result,
            "L01\nL02\nL03\nL04\nL06\nL07\nL08\nL09\nS10\nL11\nL12"
        );
    }

    #[test]
    fn bug104_del_range_after_swap() {
        // Case 2: DEL 5..7 after SWAP 10 — L05..L07 removed, not L10..L12.
        let result = apply_text(L12, "SWAP 10:\n+S10\nDEL 5..7");
        assert_eq!(result, "L01\nL02\nL03\nL04\nL08\nL09\nS10\nL11\nL12");
    }

    #[test]
    fn bug104_del_after_multi_line_swap() {
        // Case 3: DEL 5 after a two-line SWAP body.
        let result = apply_text(L12, "SWAP 10:\n+S10a\n+S10b\nDEL 5");
        assert_eq!(
            result,
            "L01\nL02\nL03\nL04\nL06\nL07\nL08\nL09\nS10a\nS10b\nL11\nL12"
        );
    }

    #[test]
    fn bug104_del_after_swap_range() {
        // Case 4: DEL 5 after SWAP 9..10 — range length and delete count
        // must not sum; L09..L10 → S9 and L05 removed.
        let result = apply_text(L12, "SWAP 9..10:\n+S9\nDEL 5");
        assert_eq!(result, "L01\nL02\nL03\nL04\nL06\nL07\nL08\nS9\nL11\nL12");
    }

    #[test]
    fn bug104_del_after_interleaved_swaps() {
        // Case 5: SWAP 12, SWAP 10, then DEL 5 — both SWAPs apply, L05
        // removed, L11 intact (L11 must not be consumed by the last SWAP).
        let result = apply_text(L12, "SWAP 12:\n+S12\nSWAP 10:\n+S10\nDEL 5");
        assert_eq!(
            result,
            "L01\nL02\nL03\nL04\nL06\nL07\nL08\nL09\nS10\nL11\nS12"
        );
    }

    #[test]
    fn bug104_two_dels_after_one_swap() {
        // Case 6: DEL 5 and DEL 3 after one SWAP — both must land.
        let result = apply_text(L12, "SWAP 10:\n+S10\nDEL 5\nDEL 3");
        assert_eq!(result, "L01\nL02\nL04\nL06\nL07\nL08\nL09\nS10\nL11\nL12");
    }

    #[test]
    fn bug104_del_blk_two_anchor_after_swap() {
        // Case 7: DEL.BLK 5 7 (two-anchor → concrete range) after SWAP 10.
        let result = apply_text(L12, "SWAP 10:\n+S10\nDEL.BLK 5 7");
        assert_eq!(result, "L01\nL02\nL03\nL04\nL08\nL09\nS10\nL11\nL12");
    }

    #[test]
    fn bug104_del_after_swap_wrong_hash_guard() {
        // Case 8: validation and mutation must resolve the same line — a
        // stale hash on the DEL anchor errors on line 5 and mutates nothing.
        let err = apply_text_result(L12, "SWAP 10:\n+S10\nDEL 5:AA", "md").unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error, got: {err}"
        );
    }

    // =====================================================================
    // P9: named-register clipboard ops (CUT @name / PUT @name)
    // =====================================================================

    #[test]
    fn p9_cut_then_put_moves_range_to_new_position() {
        // CUT 2..3 @fn captures lines 2..3 and deletes them; PUT @fn <5
        // re-inserts them before original line 5.
        let result = apply_text("L01\nL02\nL03\nL04\nL05\nL06", "CUT 2..3 @fn\nPUT @fn <5");
        assert_eq!(result, "L01\nL04\nL02\nL03\nL05\nL06");
    }

    #[test]
    fn p9_cut_then_put_moves_range_to_file_head() {
        // Bare `PUT @fn` (no cursor) defaults to file head.
        let result = apply_text("L01\nL02\nL03\nL04", "CUT 3 @fn\nPUT @fn");
        assert_eq!(result, "L03\nL01\nL02\nL04");
    }

    #[test]
    fn p9_cut_without_put_deletes() {
        // CUT is a DEL that stores the removed content; with no matching PUT
        // the lines are gone.
        let result = apply_text("L01\nL02\nL03\nL04", "CUT 2..3 @fn");
        assert_eq!(result, "L01\nL04");
    }

    #[test]
    fn p9_cut_anonymous_register_paste() {
        // CUT without @name fills the anonymous register; an unlabeled PUT
        // pastes it back before original line 5.
        let result = apply_text("L01\nL02\nL03\nL04\nL05", "CUT 2..3\nPUT <5");
        assert_eq!(result, "L01\nL04\nL02\nL03\nL05");
    }

    #[test]
    fn p9_put_never_captured_register_errors() {
        // PUT referencing a register that was never captured in the same
        // patch must hard-error (fail-closed).
        let err = apply_text_result("L01\nL02\nL03\nL04", "PUT @nosuch <2", "txt").unwrap_err();
        assert!(
            err.to_string().contains("never captured"),
            "expected ClipboardMissingRegister error, got: {err}"
        );
    }

    #[test]
    fn p9_anonymous_put_without_any_cut_errors() {
        let err = apply_text_result("L01\nL02\nL03\nL04", "PUT <2", "txt").unwrap_err();
        assert!(
            err.to_string().contains("anonymous register is empty"),
            "expected ClipboardEmptyAnon error, got: {err}"
        );
    }

    #[test]
    fn p9_cut_validates_anchor_hash() {
        // Wrong hash on the CUT anchor → StaleAnchor, like DEL.
        let err = apply_text_result("L01\nL02\nL03\nL04", "CUT 2:FF..3 @fn", "txt").unwrap_err();
        assert!(
            err.to_string().contains("content changed since last read"),
            "expected StaleAnchor error, got: {err}"
        );
    }

    #[test]
    fn p9_cut_with_valid_anchor_hash_succeeds() {
        let h = crate::hash::format_short_hash(crate::hash::short_hash_value("L02"));
        let result = apply_text("L01\nL02\nL03\nL04", &format!("CUT 2:{h} @fn\nPUT @fn <4"));
        assert_eq!(result, "L01\nL03\nL02\nL04");
    }

    #[test]
    fn p9_cut_out_of_range_errors() {
        let err = apply_text_result("L01\nL02\nL03", "CUT 5..6 @fn", "txt").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected InvalidAnchor error, got: {err}"
        );
    }

    #[test]
    fn p9_put_out_of_range_anchor_errors() {
        let err = apply_text_result("L01\nL02\nL03", "CUT 1 @fn\nPUT @fn <99", "txt").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected InvalidAnchor error, got: {err}"
        );
    }

    #[test]
    fn p9_cut_then_put_then_swap_sequencing() {
        // CUT shifts the buffer; a later SWAP must resolve its anchor against
        // the shifted state.
        let result = apply_text(
            "L01\nL02\nL03\nL04\nL05",
            "CUT 2..3 @fn\nPUT @fn <5\nSWAP 5:\n+S5",
        );
        assert_eq!(result, "L01\nL04\nL02\nL03\nS5");
    }

    #[test]
    fn p9_cut_after_swap_captures_original_lines() {
        // SWAP 10 rewrites L10, then CUT 10 captures the ORIGINAL L10 content
        // (from the snapshot), not the replacement. The CUT's delete of
        // original line 10 is a no-op because the SWAP already consumed it
        // (same `!deleted` guard as DEL); the captured L10 is pasted at the
        // head, and the SWAP's S10 survives in place.
        let result = apply_text(L12, "SWAP 10:\n+S10\nCUT 10 @fn\nPUT @fn <2");
        assert_eq!(
            result,
            "L01\nL10\nL02\nL03\nL04\nL05\nL06\nL07\nL08\nL09\nS10\nL11\nL12"
        );
    }

    #[test]
    fn p9_cut_after_swap_does_not_delete_swap_anchor() {
        // CUT 10 after SWAP 10 must not delete the line the SWAP just
        // rewrote — the `!deleted` guard skips it, and the captured lines
        // are the ORIGINAL ones. L10 stays (rewritten to S10), only L11
        // is deleted by the CUT.
        let result = apply_text(L12, "SWAP 10:\n+S10\nCUT 11 @fn\nPUT @fn <2");
        assert_eq!(
            result,
            "L01\nL11\nL02\nL03\nL04\nL05\nL06\nL07\nL08\nL09\nS10\nL12"
        );
    }

    // ---- Bug #106: INS.PRE / INS.POST ignore shift table in multi-edit patches ----

    #[test]
    fn bug106_multi_ins_post_compounding_drift() {
        // Three INS.POST at different anchors should each land at the
        // correct position without compounding drift.
        assert_eq!(
            apply_text("L1\nL2\nL3\nL4\nL5\nL6", "INS.POST 2:\n+A\nINS.POST 4:\n+B"),
            "L1\nL2\nA\nL3\nL4\nB\nL5\nL6"
        );
    }

    #[test]
    fn bug106_del_before_ins_post() {
        // A DEL before an INS.POST should not cause the insert to land
        // one line late.
        assert_eq!(
            apply_text("L1\nL2\nL3\nL4\nL5\nL6", "DEL 2\nINS.POST 4:\n+X"),
            "L1\nL3\nL4\nX\nL5\nL6"
        );
    }

    #[test]
    fn bug106_multi_ins_pre() {
        // Two INS.PRE at different anchors should each land before the
        // correct original line.
        assert_eq!(
            apply_text("L1\nL2\nL3\nL4\nL5", "INS.PRE 2:\n+P\nINS.PRE 5:\n+Q"),
            "L1\nP\nL2\nL3\nL4\nQ\nL5"
        );
    }

    #[test]
    fn bug106_ins_blk_post_multi_block() {
        // Two INS.BLK.POST at different function anchors should inject
        // comments after each function body, not inside the next one.
        let original = "fn one() {\n    let a = 1;\n}\nfn two() {\n    let b = 2;\n}\nfn three() {\n    let c = 3;\n}\n";
        let patch = "INS.BLK.POST 1:\n+// after fn one\nINS.BLK.POST 4:\n+// after fn two\n";
        let result = apply_text_ext(original, patch, "rs");
        assert_eq!(
            result,
            "fn one() {\n    let a = 1;\n}\n// after fn one\nfn two() {\n    let b = 2;\n}\n// after fn two\nfn three() {\n    let c = 3;\n}\n"
        );
    }
    // =====================================================================
    // Regression tests for Issue #112 — unknown op keywords must cause
    // atomic failure, not partial file corruption.
    // =====================================================================

    #[test]
    fn bug112_unknown_op_with_pending_swap_aborts_atomically() {
        // A valid SWAP followed by an unknown keyword (`END`) must reject
        // the entire patch — no file mutation, no partial apply.
        let (edits, warnings, _file_op, aborted) =
            crate::parser::parse_patch("SWAP 2:\n+replaced\nEND");
        assert!(edits.is_empty(), "expected zero edits, got {edits:?}");
        assert!(aborted, "expected aborted=true when unknown op is present");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown operation `END`")),
            "expected unknown op warning, got {warnings:?}"
        );
    }

    #[test]
    fn bug112_unknown_op_between_valid_ops_aborts_atomically() {
        // Unknown keyword between two valid ops — entire patch must be rejected.
        let (edits, warnings, _file_op, aborted) =
            crate::parser::parse_patch("SWAP 2:\n+replaced\nEND\nDEL 3");
        assert!(edits.is_empty(), "expected zero edits, got {edits:?}");
        assert!(aborted, "expected aborted=true");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown operation `END`")),
            "expected unknown op warning, got {warnings:?}"
        );
    }

    #[test]
    fn bug112_standalone_unknown_op_rejected() {
        // A standalone unknown keyword (no pending op) must also be rejected.
        let (edits, _warnings, _file_op, aborted) =
            crate::parser::parse_patch("SWAP 1:\n+foo\nEND");
        assert!(edits.is_empty(), "expected zero edits, got {edits:?}");
        assert!(aborted, "expected aborted=true");
    }

    #[test]
    fn bug112_valid_patch_still_works() {
        // Sanity: a valid patch with no unknown ops must still apply.
        let result = apply_text("line1\nline2\nline3", "SWAP 2:\n+replaced2");
        assert_eq!(result, "line1\nreplaced2\nline3");
    }

    #[test]
    fn bug112_plus_prefixed_uppercase_payload_not_rejected() {
        // `+REPLACED` is a legitimate payload (explicit `+` prefix) — must NOT
        // be flagged as unknown op. Only bare (unprefixed) uppercase keywords
        // trigger the abort.
        let result = apply_text("line1\nline2\nline3", "SWAP 2:\n+REPLACED");
        assert_eq!(result, "line1\nREPLACED\nline3");
    }

    // ---- Issue #119: opt-in updated anchors ----

    /// Drive the full CLI `patch::run` against a temp file and capture stdout.
    fn run_patch_stdout(
        dir: &std::path::Path,
        name: &str,
        content: &str,
        patch_text: &str,
        emit_anchors: bool,
        json: bool,
    ) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        let cmd = crate::cli::PatchCmd {
            file: path.clone(),
            patch: patch_text.to_string(),
            dry_run: false,
            safe: false,
            json,
            verbose: false,
            emit_anchors,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = crate::context::CommandContext::new(
            &mut stdout,
            &mut stderr,
            crate::context::output_mode_for(&crate::cli::Commands::Patch(cmd.clone())),
        );
        super::run(&mut ctx, cmd).expect("patch should succeed");
        String::from_utf8(stdout).unwrap()
    }

    #[test]
    fn issue119_default_output_has_no_anchor_block() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_stdout(
            dir.path(),
            "a.txt",
            "line one\nline two\nline three\n",
            "SWAP 2:\n+replaced",
            false,
            false,
        );
        assert!(out.starts_with("OK "), "{out}");
        assert!(!out.contains("[a.txt#") && !out.contains("]#"), "{out}");
    }

    #[test]
    fn issue119_emit_anchors_lists_fresh_anchors_for_chain() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_stdout(
            dir.path(),
            "a.txt",
            "line one\nline two\nline three\n",
            "INS.POST 2:\n+inserted line",
            true,
            false,
        );
        // Summary first, then a read-compatible full anchor listing.
        let header = out.lines().find(|l| l.starts_with('[')).expect("{out}");
        let anchors: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.starts_with('['))
            .skip(1)
            .collect();
        assert_eq!(anchors.len(), 4, "{out}");
        // Line numbers are fresh post-edit positions...
        assert!(anchors[2].starts_with("3:"), "{out}");
        assert!(anchors[3].starts_with("4:"), "{out}");
        // ...and the listing parses as `N:hh|content`.
        for a in &anchors {
            assert!(a.contains(':') && a.contains('|'), "{a}");
        }
        // The shifted line keeps its content+hash, so the anchor is reusable.
        assert!(anchors[3].ends_with("|line three"), "{out}");
        let _ = header;
    }

    #[test]
    fn issue119_emit_anchors_matches_read_for_symbol_lines() {
        // Symbol-only lines are position-seeded: the emitted anchors must
        // equal what a fresh `read` reports, or chained edits go stale.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.rs");
        std::fs::write(&path, "fn f() {\n    let x = 1;\n}\n").unwrap();
        let cmd = crate::cli::PatchCmd {
            file: path.clone(),
            patch: "INS.POST 1:\n+// comment".to_string(),
            dry_run: false,
            safe: false,
            json: false,
            verbose: false,
            emit_anchors: true,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = crate::context::CommandContext::new(
            &mut stdout,
            &mut stderr,
            crate::context::OutputMode::Compact,
        );
        super::run(&mut ctx, cmd).expect("patch should succeed");
        let emitted = String::from_utf8(stdout).unwrap();

        let read_cmd = crate::cli::ReadCmd {
            file: path,
            json: false,
            no_cache: false,
        };
        let mut r_out = Vec::new();
        let mut r_err = Vec::new();
        let mut r_ctx = crate::context::CommandContext::new(
            &mut r_out,
            &mut r_err,
            crate::context::OutputMode::Compact,
        );
        crate::commands::read::run(&mut r_ctx, read_cmd).unwrap();
        let read_out = String::from_utf8(r_out).unwrap();

        let emitted_block = emitted
            .split_once('[')
            .map(|(_, rest)| format!("[{rest}"))
            .unwrap();
        assert_eq!(emitted_block.trim_end(), read_out.trim_end());
    }

    #[test]
    fn issue119_json_emit_adds_updated_anchors_array() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_stdout(
            dir.path(),
            "a.txt",
            "line one\nline two\n",
            "SWAP 2:\n+replaced",
            true,
            true,
        );
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        let anchors = v.get("updated_anchors").expect("{v}");
        assert_eq!(anchors.as_array().unwrap().len(), 2, "{v}");
        assert_eq!(anchors[1]["n"], 2);
        assert_eq!(anchors[1]["content"], "replaced");
        assert!(anchors[1]["hash"].as_str().unwrap().len() == 2);
    }

    #[test]
    fn issue119_json_default_has_no_updated_anchors_key() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_stdout(
            dir.path(),
            "a.txt",
            "line one\nline two\n",
            "SWAP 2:\n+replaced",
            false,
            true,
        );
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert!(v.get("updated_anchors").is_none(), "{v}");
    }

    // ---- ChangeSet classification (bd-his) --------------------------------
    //
    // A content-match heuristic cannot tell "insert a new line" from
    // "rewrite that line" — both look like one line replaced by one
    // different line. The heuristic reported the *inserted* line as
    // modified and an *untouched* neighbour as inserted, so the `~`/`+`
    // prefixes and the `changed=` count were wrong for every insert
    // operation. These tests pin the classification to the operation that
    // actually ran.

    /// Apply a patch and return the resulting [`ChangeSet`].
    fn apply_changeset(original: &str, patch_text: &str) -> ChangeSet {
        apply_changeset_ext(original, patch_text, "rs")
    }

    fn apply_changeset_ext(original: &str, patch_text: &str, ext: &str) -> ChangeSet {
        let (edits, _warnings, _file_op, _aborted) = parse_patch(patch_text);
        let mut lines: Vec<String> = original.split('\n').map(|s| s.to_string()).collect();
        let path_str = format!("test.{ext}");
        let entries_with_content: Vec<crate::document::LineEntry> = lines
            .iter()
            .map(|s| crate::document::LineEntry {
                content: s.clone(),
                short_hash: crate::hash::short_hash_value(s),
            })
            .collect();
        apply_edits_tracked(
            &mut lines,
            &entries_with_content,
            std::path::Path::new(&path_str),
            &edits,
            count_operations(&edits),
            original.ends_with('\n'),
            true,
        )
        .expect("edit should succeed")
    }

    #[test]
    fn changeset_reports_insert_post_as_inserted() {
        let cs = apply_changeset("line1\nline2\nline3", "INS.POST 1:\n+NEW");
        assert_eq!(cs.inserted.len(), 1, "one line inserted: {cs:?}");
        assert_eq!(cs.inserted[0].content, "NEW");
        assert_eq!(cs.inserted[0].line, 2);
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
        assert!(cs.deleted.is_empty(), "nothing deleted: {cs:?}");
    }

    #[test]
    fn changeset_reports_insert_pre_as_inserted() {
        let cs = apply_changeset("line1\nline2\nline3", "INS.PRE 2:\n+NEW");
        assert_eq!(cs.inserted.len(), 1, "one line inserted: {cs:?}");
        assert_eq!(cs.inserted[0].content, "NEW");
        assert_eq!(cs.inserted[0].line, 2);
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
        assert!(cs.deleted.is_empty(), "nothing deleted: {cs:?}");
    }

    #[test]
    fn changeset_reports_insert_head_as_inserted() {
        let cs = apply_changeset("line1\nline2\nline3", "INS.HEAD:\n+NEW");
        assert_eq!(cs.inserted.len(), 1, "one line inserted: {cs:?}");
        assert_eq!(cs.inserted[0].content, "NEW");
        assert_eq!(cs.inserted[0].line, 1);
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
        assert!(cs.deleted.is_empty(), "nothing deleted: {cs:?}");
    }

    #[test]
    fn changeset_reports_swap_as_modified() {
        let cs = apply_changeset("line1\nline2\nline3", "SWAP 2:\n+REPLACED");
        assert_eq!(cs.modified.len(), 1, "one line rewritten: {cs:?}");
        assert_eq!(cs.modified[0].content, "REPLACED");
        assert_eq!(cs.modified[0].line, 2);
        assert!(cs.inserted.is_empty(), "nothing inserted: {cs:?}");
        assert!(cs.deleted.is_empty(), "nothing deleted: {cs:?}");
    }

    #[test]
    fn changeset_reports_delete_as_deleted() {
        let cs = apply_changeset("line1\nline2\nline3", "DEL 2");
        assert_eq!(cs.deleted, vec![2], "one line deleted: {cs:?}");
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
        assert!(cs.inserted.is_empty(), "nothing inserted: {cs:?}");
    }

    #[test]
    fn changeset_reports_block_swap_as_modified_plus_deleted() {
        let original = "fn outer() {\n    let a = 1;\n    let b = 2;\n}\nfn other() {}";
        let cs = apply_changeset_ext(original, "SWAP.BLK 1:\n+fn replaced() {}", "rs");
        assert_eq!(cs.modified.len(), 1, "block replaced by one line: {cs:?}");
        assert_eq!(cs.modified[0].content, "fn replaced() {}");
        assert_eq!(cs.deleted.len(), 3, "rest of the block deleted: {cs:?}");
        assert!(cs.inserted.is_empty(), "nothing inserted: {cs:?}");
    }

    #[test]
    fn changeset_reports_move_as_insert_plus_delete() {
        // CUT + PUT relocates a line: it is an insertion at the new spot and
        // a deletion at the old one, not "nothing changed".
        let cs = apply_changeset("aaa\nbbb\nccc\nddd", "CUT 4 @fn\nPUT @fn <1");
        assert_eq!(cs.inserted.len(), 1, "line pasted: {cs:?}");
        assert_eq!(cs.inserted[0].content, "ddd");
        assert_eq!(cs.inserted[0].line, 1);
        assert_eq!(
            cs.deleted,
            vec![4],
            "line removed from its old spot: {cs:?}"
        );
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
    }

    #[test]
    fn changeset_reports_a_swapped_pair_as_one_delete_and_one_insert() {
        // Swapping two lines admits two equally-valid minimal edit scripts —
        // either line can be the one that "moved". The diff picks one; what
        // matters is that the change is reported at all rather than as
        // "nothing changed".
        let cs = apply_changeset("line1\nline2\nline3\nline4", "CUT 2 @fn\nPUT @fn <1");
        assert_eq!(cs.inserted.len(), 1, "one line pasted: {cs:?}");
        assert_eq!(cs.deleted.len(), 1, "one line removed: {cs:?}");
        assert!(cs.modified.is_empty(), "nothing rewritten: {cs:?}");
    }

    #[test]
    fn changeset_changed_count_matches_classified_lines() {
        for patch in [
            "INS.POST 1:\n+NEW",
            "INS.PRE 2:\n+NEW",
            "INS.HEAD:\n+NEW",
            "SWAP 2:\n+REPLACED",
            "DEL 2",
        ] {
            let cs = apply_changeset("line1\nline2\nline3", patch);
            let classified = cs.modified.len() + cs.inserted.len() + cs.deleted.len();
            assert_eq!(classified, 1, "one line changed for {patch:?}: {cs:?}");
        }
    }

    // ---- Operation counting (bd-41p) --------------------------------------

    #[test]
    fn count_operations_counts_operations_not_edits() {
        // A three-line SWAP body expands into three Insert edits plus a
        // Delete, but it is one operation. The CLI and the MCP server must
        // report the same number.
        let (edits, _warnings, _file_op, _aborted) = parse_patch("SWAP 2:\n+a\n+b\n+c");
        assert!(
            edits.len() > 1,
            "the body should expand into several edits, got {:?}",
            edits.len()
        );
        assert_eq!(count_operations(&edits), 1);
    }

    #[test]
    fn count_operations_counts_distinct_operations() {
        let (edits, _warnings, _file_op, _aborted) = parse_patch("SWAP 1:\n+x\nDEL 3");
        assert_eq!(count_operations(&edits), 2);
    }

    // ---- Rejected patches must fail (bd-5x5) ------------------------------

    /// Run the patch command and return the error it produced, if any.
    fn run_patch_result(
        dir: &std::path::Path,
        name: &str,
        content: &str,
        patch_text: &str,
    ) -> Result<String, HashlineError> {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        let cmd = crate::cli::PatchCmd {
            file: path.clone(),
            patch: patch_text.to_string(),
            dry_run: false,
            safe: false,
            json: false,
            verbose: false,
            emit_anchors: false,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = crate::context::CommandContext::new(
            &mut stdout,
            &mut stderr,
            crate::context::OutputMode::Compact,
        );
        super::run(&mut ctx, cmd)?;
        Ok(String::from_utf8(stdout).unwrap())
    }

    #[test]
    fn rejected_patch_with_unknown_op_fails() {
        let dir = tempfile::tempdir().unwrap();
        let original = "line1\nline2\n";
        let err = run_patch_result(dir.path(), "a.txt", original, "TOTALLY BOGUS")
            .expect_err("an unknown operation must be an error, not a silent success");
        assert_eq!(err.kind(), "PATCH_REJECTED");
        // The file must be untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            original
        );
    }

    #[test]
    fn rejected_patch_with_malformed_op_fails() {
        let dir = tempfile::tempdir().unwrap();
        let original = "line1\nline2\n";
        let err = run_patch_result(dir.path(), "a.txt", original, "PUT 1:\n+payload")
            .expect_err("a malformed operation must be an error");
        assert_eq!(err.kind(), "PATCH_REJECTED");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            original
        );
    }

    #[test]
    fn rejected_patch_mixing_valid_and_unknown_ops_fails() {
        let dir = tempfile::tempdir().unwrap();
        let original = "line1\nline2\n";
        let err = run_patch_result(
            dir.path(),
            "a.txt",
            original,
            "SWAP 1:\n+VALID\nBOGUSOP 2:\n+X",
        )
        .expect_err("a patch mixing valid and unknown operations must be rejected");
        assert_eq!(err.kind(), "PATCH_REJECTED");
        // The whole patch is rejected atomically — the valid op must not land.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            original
        );
    }

    #[test]
    fn empty_patch_still_reports_empty_patch() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_patch_result(dir.path(), "a.txt", "line1\n", "")
            .expect_err("an empty patch must be an error");
        assert_eq!(err.kind(), "EMPTY_PATCH");
    }

    // =====================================================================
    // Gap-model position tracking regressions (bd-patch-shift-table-3r2)
    // =====================================================================

    #[test]
    fn noncontiguous_dels_do_not_corrupt_later_ops() {
        // Two far-apart deletes used to collapse into one shift delta and
        // misplace every subsequent op.
        let result = apply_text(
            "L1\nL2\nL3\nL4\nL5\nL6\nL7",
            "DEL 2\nDEL 6\nSWAP 7:\n+END\nINS.POST 4:\n+X",
        );
        // DEL 2 and DEL 6 removed; SWAP 7 rewrites L7; INS.POST 4 lands X.
        assert_eq!(result, "L1\nL3\nL4\nX\nL5\nEND");
    }

    #[test]
    fn duplicate_delete_is_rejected() {
        let err = apply_text_result("L1\nL2\nL3", "DEL 2\nDEL 2", "txt").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
    }

    #[test]
    fn overlapping_delete_range_is_rejected() {
        let err = apply_text_result("L1\nL2\nL3\nL4\nL5", "DEL 2..4\nDEL 3", "txt").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
    }

    #[test]
    fn swap_range_over_already_deleted_line_is_rejected() {
        // SWAP 2..4 after DEL 3: the old code ignored deleted[] for range
        // members and ate an extra unintended line.
        let err =
            apply_text_result("L1\nL2\nL3\nL4\nL5", "DEL 3\nSWAP 2..4:\n+x", "txt").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
    }

    #[test]
    fn interleaved_ins_del_at_same_anchor() {
        // The verified corruption: INS.POST 50 + DEL 50 + INS.POST 50 +
        // DEL 51 used to make DEL 51 remove the just-inserted payload.
        // Scaled down: inserted payloads must survive and DEL 51 must
        // remove ORIGINAL line 51.
        let original = (1..=60)
            .map(|i| format!("L{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = apply_text(
            &original,
            "INS.POST 50:\n+INS1\nDEL 50\nINS.POST 50:\n+INS2\nDEL 51",
        );
        let lines: Vec<&str> = result.split('\n').collect();
        assert!(lines.contains(&"INS1"), "{result}");
        assert!(lines.contains(&"INS2"), "{result}");
        assert!(!lines.contains(&"L50"), "{result}");
        assert!(!lines.contains(&"L51"), "{result}");
        assert!(lines.contains(&"L52"), "{result}");
        // Both inserts sit between L49 and L52, in patch order.
        let i49 = lines.iter().position(|&l| l == "L49").unwrap();
        assert_eq!(&lines[i49 + 1..i49 + 3], &["INS1", "INS2"]);
    }

    #[test]
    fn interleaved_ins_del_storm() {
        // 200-op storm of the same shape across many anchors — the old
        // shift table misfired almost everywhere (changed=1).
        let n = 200;
        let original = (1..=n)
            .map(|i| format!("L{i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut patch = String::new();
        for k in (10..190).step_by(10) {
            patch.push_str(&format!(
                "INS.POST {k}:\n+INS{k}\nDEL {k}\nINS.POST {k}:\n+INS{k}b\nDEL {}\n",
                k + 1
            ));
        }
        let result = apply_text(&original, &patch);
        let lines: Vec<&str> = result.split('\n').collect();
        for k in (10..190).step_by(10) {
            assert!(
                lines.contains(&format!("INS{k}").as_str()),
                "missing INS{k}"
            );
            assert!(
                lines.contains(&format!("INS{k}b").as_str()),
                "missing INS{k}b"
            );
            assert!(
                !lines.contains(&format!("L{k:03}").as_str()),
                "L{k} should be deleted"
            );
            assert!(
                !lines.contains(&format!("L{:03}", k + 1).as_str()),
                "L{} should be deleted",
                k + 1
            );
        }
        // Spot-check an untouched line survives.
        assert!(lines.contains(&"L001"));
        assert!(lines.contains(&"L200"));
    }

    #[test]
    fn del_blk_trailing_blank_is_marked_deleted() {
        // fn block (lines 1-2), blank line 3, second fn (4-5).
        // DEL.BLK 1 consumes the blank separator; a later DEL 3 must now
        // be rejected as an overlap instead of silently misplacing.
        let original = "fn a() {\n}\n\nfn b() {\n}\n";
        let err = apply_text_result(original, "DEL.BLK 1\nDEL 3", "rs").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
        // And the happy path: block + blank removed, second fn intact.
        let result = apply_text(original, "DEL.BLK 1");
        assert_eq!(result, "fn b() {\n}\n");
    }

    #[test]
    fn block_op_overlapping_prior_edit_is_rejected() {
        // SWAP 2 rewrites a line inside fn a's block; DEL.BLK 1 must not
        // retarget against the stale original span.
        let original = "fn a() {\n    let x = 1;\n}\nfn b() {\n}\n";
        let err =
            apply_text_result(original, "SWAP 2:\n+    let y = 2;\nDEL.BLK 1", "rs").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
    }

    #[test]
    fn swap_payload_sits_in_content_slot_not_gap() {
        // INS.PRE lands before the replacement, INS.POST after it —
        // regardless of op order in the patch.
        assert_eq!(
            apply_text("a\nb\nc", "SWAP 2:\n+X\nINS.PRE 2:\n+P\nINS.POST 2:\n+Q"),
            "a\nP\nX\nQ\nc"
        );
        assert_eq!(
            apply_text("a\nb\nc", "INS.PRE 2:\n+P\nINS.POST 2:\n+Q\nSWAP 2:\n+X"),
            "a\nP\nX\nQ\nc"
        );
    }

    // ---- changeset hash + fallback flood (bd-changeset-hash-6fu) ----

    #[test]
    fn changeset_hash_matches_read_for_no_trailing_newline() {
        use crate::hash::compute_file_hash;
        // No trailing newline: reported hash must be over "x\nb" not "x\nb\n".
        let cs = apply_changeset_ext("a\nb", "SWAP 1:\n+x", "txt");
        assert_eq!(cs.file_hash, compute_file_hash("x\nb"));
        assert_ne!(cs.file_hash, compute_file_hash("x\nb\n"));
        // With trailing newline the hash includes it — drive the same
        // split_normalized path `run()` uses (the changeset helper above
        // keeps the phantom trailing line, which would double-count).
        let (edits, _w, _f, _a) = parse_patch("SWAP 1:\n+x");
        let mut lines = split_normalized("a\nb\n");
        let entries: Vec<crate::document::LineEntry> = lines
            .iter()
            .enumerate()
            .map(|(i, s)| crate::document::LineEntry {
                content: s.clone(),
                short_hash: crate::hash::short_hash_value_indexed(s, i + 1),
            })
            .collect();
        let cs = apply_edits_tracked(
            &mut lines,
            &entries,
            Path::new("t.txt"),
            &edits,
            1,
            true,
            true,
        )
        .unwrap();
        assert_eq!(cs.file_hash, compute_file_hash("x\nb\n"));
    }

    #[test]
    fn changeset_sparse_deletes_on_huge_file_stay_small() {
        // 10k-line file, two deletes at the extremes. The old LCS budget
        // fallback paired ~10k lines as "modified" (changed=9999 flood).
        let original = (1..=10_000)
            .map(|i| format!("L{i:05}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cs = apply_changeset_ext(&original, "DEL 1\nDEL 9999", "txt");
        assert_eq!(cs.deleted, vec![1, 9999], "{cs:?}");
        assert!(cs.modified.is_empty(), "{cs:?}");
        assert!(cs.inserted.is_empty(), "{cs:?}");
        let changed = cs.modified.len() + cs.inserted.len() + cs.deleted.len();
        assert!(changed <= 10, "changeset must stay small, got {changed}");
    }

    #[test]
    fn changeset_truncates_genuinely_huge_rewrite() {
        // A wholesale rewrite bigger than the report cap is bounded, with a
        // warning — never a 50k-line flood.
        let n = 3000;
        let original = (1..=n)
            .map(|i| format!("OLD{i:04}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut patch = String::from("SWAP 1..");
        patch.push_str(&n.to_string());
        patch.push_str(":\n");
        for i in 1..=n {
            patch.push_str(&format!("+NEW{i:04}\n"));
        }
        let cs = apply_changeset_ext(&original, &patch, "txt");
        assert!(cs.modified.len() <= 1000, "{}", cs.modified.len());
        assert!(cs.deleted.len() <= 1000, "{}", cs.deleted.len());
    }

    // ---- file ops (bd-rem-mv-ops-pe8) ----

    #[test]
    fn patch_rem_deletes_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_result(dir.path(), "a.txt", "content\n", "REM").unwrap();
        assert!(out.starts_with("OK "), "{out}");
        assert!(!dir.path().join("a.txt").exists());
    }

    #[test]
    fn patch_mv_renames_file_relative_to_source_dir() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_result(dir.path(), "a.txt", "content\n", "MV b.txt").unwrap();
        assert!(out.starts_with("OK "), "{out}");
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "content\n"
        );
    }

    #[test]
    fn patch_mv_onto_existing_target_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "other\n").unwrap();
        let err = run_patch_result(dir.path(), "a.txt", "content\n", "MV b.txt").unwrap_err();
        assert_eq!(err.kind(), "TARGET_EXISTS", "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "content\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "other\n"
        );
    }

    #[test]
    fn patch_file_op_combined_with_edits_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            run_patch_result(dir.path(), "a.txt", "l1\nl2\n", "REM\nSWAP 1:\n+x").unwrap_err();
        assert_eq!(err.kind(), "PATCH_REJECTED", "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "l1\nl2\n"
        );
    }

    // ---- write-time stale-file guard (bd-concurrency-toctou-ch6) ----

    #[test]
    fn drifted_source_is_rejected_before_write() {
        // Simulates the lost-update repro: the patch loads the file, another
        // writer swaps in a decoy, and the write must fail closed instead of
        // destroying the decoy's content.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "original\n").unwrap();
        let loaded = FileContent::load(&path).unwrap();

        // No drift → the guard passes.
        super::reject_if_source_drifted(&path, &loaded).unwrap();

        // A rewrite between load and write → STALE_FILE.
        std::fs::write(&path, "decoy\n").unwrap();
        let err = super::reject_if_source_drifted(&path, &loaded).unwrap_err();
        assert_eq!(err.kind(), "STALE_FILE", "{err}");
        // The decoy must be untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "decoy\n");
    }

    #[test]
    fn drifted_source_via_delete_and_recreate_is_rejected() {
        // Same guard, but the "swap" is delete + recreate with different
        // content — metadata identity is irrelevant, content is compared.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "original\n").unwrap();
        let loaded = FileContent::load(&path).unwrap();

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "original\n").unwrap(); // same content → ok
        super::reject_if_source_drifted(&path, &loaded).unwrap();

        std::fs::write(&path, "changed\n").unwrap();
        let err = super::reject_if_source_drifted(&path, &loaded).unwrap_err();
        assert_eq!(err.kind(), "STALE_FILE", "{err}");
    }

    #[test]
    fn encoding_only_drift_is_rejected() {
        // An encoding-only change (BOM added, EOL style flipped, trailing
        // newline added) produces IDENTICAL normalized text — the drift
        // check must compare raw bytes or it silently lets the patch
        // clobber the other writer's encoding change
        // (bd-write-safety-batch-sz62).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let loaded = FileContent::load(&path).unwrap();

        // Same visible text, but CRLF endings — normalized text is
        // unchanged, raw bytes differ.
        std::fs::write(&path, "line1\r\nline2\r\n").unwrap();
        let err = super::reject_if_source_drifted(&path, &loaded).unwrap_err();
        assert_eq!(err.kind(), "STALE_FILE", "{err}");

        // Restore, then add a BOM — again normalized-identical.
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let loaded = FileContent::load(&path).unwrap();
        std::fs::write(&path, "\u{feff}line1\nline2\n").unwrap();
        let err = super::reject_if_source_drifted(&path, &loaded).unwrap_err();
        assert_eq!(err.kind(), "STALE_FILE", "{err}");
    }

    // ---- write_patch_output: drift guard + post-write verification ----
    // (bd-concurrency-residual-ubpl)

    #[test]
    fn write_patch_output_writes_and_byte_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "original\n").unwrap();
        let fc = FileContent::load(&path).unwrap();

        for safe in [false, true] {
            super::write_patch_output(&path, "patched\n", &fc, safe).unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "patched\n",
                "safe={safe}"
            );
            // Restore for the next iteration.
            std::fs::write(&path, "original\n").unwrap();
        }
    }

    #[test]
    fn write_patch_output_rejects_drifted_source() {
        // A decoy swapped in after load must survive untouched in BOTH
        // modes — including --safe, where the drift check now also runs
        // inside the persist retry loop.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "original\n").unwrap();
        let fc = FileContent::load(&path).unwrap();
        std::fs::write(&path, "decoy\n").unwrap();

        for safe in [false, true] {
            let err = super::write_patch_output(&path, "patched\n", &fc, safe).unwrap_err();
            assert_eq!(err.kind(), "STALE_FILE", "safe={safe}: {err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "decoy\n");
        }
    }

    #[test]
    fn write_patch_output_surfaces_concurrent_modification() {
        // If the file on disk no longer matches what was written when the
        // post-write verification runs, the command must fail loudly —
        // never report OK for content that isn't there. Simulated by
        // verifying against bytes that were never written.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "ours\n").unwrap();
        let fc = FileContent::load(&path).unwrap();

        // "ours\n" written via the normal path verifies fine.
        super::write_patch_output(&path, "ours\n", &fc, false).unwrap();

        // A concurrent writer lands different content; verifying OUR bytes
        // now reports CONCURRENT_MODIFICATION.
        std::fs::write(&path, "theirs\n").unwrap();
        let err = crate::commands::common::verify_written_bytes(&path, b"ours\n").unwrap_err();
        assert_eq!(err.kind(), "CONCURRENT_MODIFICATION", "{err}");
        assert_eq!(err.report_url(), None);
        // The other writer's content is preserved — we did not clobber it.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n");
    }

    // ---- fuzz: random op sequences vs an independent position oracle ----

    /// Independent oracle: the document is a sorted list of keyed elements —
    /// original lines keyed `4*line`, boundary inserts keyed `4*boundary+2`,
    /// SWAP payloads keyed `4*line-1` (the content slot between the
    /// preceding boundary and the line). Every op resolves its anchor by
    /// key ordering, never by a shift table.
    #[derive(Clone, Debug)]
    enum Elem {
        Orig(usize),
        Text(i64, String),
    }

    struct Oracle {
        doc: Vec<Elem>,
        deleted: std::collections::HashSet<usize>,
    }

    impl Oracle {
        fn new(orig: &[String]) -> Self {
            Self {
                doc: orig
                    .iter()
                    .enumerate()
                    .map(|(i, _s)| Elem::Orig(i + 1))
                    .collect(),
                deleted: std::collections::HashSet::new(),
            }
        }

        fn key(e: &Elem) -> i64 {
            match e {
                Elem::Orig(l) => 4 * *l as i64,
                Elem::Text(k, _) => *k,
            }
        }

        /// First index whose key is > `k` (upper bound).
        fn upper_bound(&self, k: i64) -> usize {
            self.doc.partition_point(|e| Self::key(e) <= k)
        }

        fn ins_boundary(&mut self, boundary: usize, text: String) {
            let pos = self.upper_bound(4 * boundary as i64 + 2);
            self.doc
                .insert(pos, Elem::Text(4 * boundary as i64 + 2, text));
        }

        fn del(&mut self, line: usize) -> Result<(), ()> {
            if self.deleted.contains(&line) {
                return Err(());
            }
            self.deleted.insert(line);
            let key = 4 * line as i64;
            if let Some(pos) = self
                .doc
                .iter()
                .position(|e| matches!(e, Elem::Orig(l) if *l == line))
            {
                self.doc.remove(pos);
                let _ = key;
            }
            Ok(())
        }

        fn swap(&mut self, start: usize, end: usize, payload: &[String]) -> Result<(), ()> {
            for l in start..=end {
                if self.deleted.contains(&l) {
                    return Err(());
                }
            }
            for l in start..=end {
                self.deleted.insert(l);
                if let Some(pos) = self
                    .doc
                    .iter()
                    .position(|e| matches!(e, Elem::Orig(x) if *x == l))
                {
                    self.doc.remove(pos);
                }
            }
            let pos = self.upper_bound(4 * start as i64 - 1);
            for (k, t) in payload.iter().enumerate() {
                self.doc
                    .insert(pos + k, Elem::Text(4 * start as i64 - 1, t.clone()));
            }
            Ok(())
        }

        fn text(&self, orig: &[String]) -> String {
            let mut out = String::new();
            for e in &self.doc {
                match e {
                    Elem::Orig(l) => out.push_str(&orig[*l - 1]),
                    Elem::Text(_, t) => out.push_str(t),
                }
                out.push('\n');
            }
            // Drop the trailing newline to match split/join comparison.
            out.pop();
            out
        }
    }

    #[test]
    fn fuzz_random_ops_match_oracle() {
        // xorshift64* — deterministic, no external deps.
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545F4914F6CDD1D)
        };

        for round in 0..400 {
            let n = 8;
            let orig: Vec<String> = (1..=n).map(|i| format!("L{i}")).collect();
            let original = orig.join("\n");
            let mut oracle = Oracle::new(&orig);
            let mut patch = String::new();
            let ops = 3 + (next() % 12) as usize;
            let mut failed = false;

            for _ in 0..ops {
                let choice = next() % 7;
                let a = 1 + (next() as usize % n);
                let b = (a + next() as usize % (n - a + 1)).min(n);
                macro_rules! payload {
                    ($tag:expr) => {
                        format!("+{}{}", $tag, next() % 1000)
                    };
                }
                match choice {
                    0 => {
                        // DEL a (or range)
                        if a == b {
                            patch.push_str(&format!("DEL {a}\n"));
                            if oracle.del(a).is_err() {
                                failed = true;
                                break;
                            }
                        } else {
                            patch.push_str(&format!("DEL {a}..{b}\n"));
                            let mut err = false;
                            for l in a..=b {
                                if oracle.del(l).is_err() {
                                    err = true;
                                    break;
                                }
                            }
                            if err {
                                failed = true;
                                break;
                            }
                        }
                    }
                    1 => {
                        // SWAP a..b with payload
                        let np = 1 + (next() % 3) as usize;
                        patch.push_str(&format!("SWAP {a}..{b}:\n"));
                        let mut texts = Vec::new();
                        for _ in 0..np {
                            let t = payload!("s");
                            patch.push_str(&t);
                            patch.push('\n');
                            texts.push(t[1..].to_string());
                        }
                        if oracle.swap(a, b, &texts).is_err() {
                            failed = true;
                            break;
                        }
                    }
                    2 => {
                        // INS.POST a
                        let t = payload!("p");
                        patch.push_str(&format!("INS.POST {a}:\n{t}\n"));
                        oracle.ins_boundary(a, t[1..].to_string());
                    }
                    3 => {
                        // INS.PRE a
                        let t = payload!("q");
                        patch.push_str(&format!("INS.PRE {a}:\n{t}\n"));
                        oracle.ins_boundary(a - 1, t[1..].to_string());
                    }
                    4 => {
                        // INS.HEAD
                        let t = payload!("h");
                        patch.push_str(&format!("INS.HEAD:\n{t}\n"));
                        oracle.ins_boundary(0, t[1..].to_string());
                    }
                    5 => {
                        // INS.TAIL
                        let t = payload!("t");
                        patch.push_str(&format!("INS.TAIL:\n{t}\n"));
                        oracle.ins_boundary(n, t[1..].to_string());
                    }
                    _ => {
                        // Single-line SWAP
                        let t = payload!("w");
                        patch.push_str(&format!("SWAP {a}:\n{t}\n"));
                        if oracle.swap(a, a, &[t[1..].to_string()]).is_err() {
                            failed = true;
                            break;
                        }
                    }
                }
            }

            if failed {
                // The patch must be rejected (overlap) — apply_edits errors.
                let (edits, _, _, aborted) = parse_patch(&patch);
                assert!(!aborted, "round {round}: unexpected abort\n{patch}");
                let mut lines = split_normalized(&original);
                let entries: Vec<crate::document::LineEntry> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, s)| crate::document::LineEntry {
                        content: s.clone(),
                        short_hash: crate::hash::short_hash_value_indexed(s, i + 1),
                    })
                    .collect();
                let err = apply_edits(&mut lines, &entries, Path::new("fuzz.txt"), &edits)
                    .expect_err("overlapping op must be rejected");
                assert_eq!(
                    err.kind(),
                    "PATCH_REJECTED",
                    "round {round}: {err}\n{patch}"
                );
                continue;
            }

            let result = apply_text(&original, &patch);
            let expected = oracle.text(&orig);
            assert_eq!(
                result, expected,
                "round {round}\npatch:\n{patch}\noriginal:\n{original}"
            );
        }
    }

    // -- bd-eol-bom-u7h: BOM / EOL round-trip through patch -----------------

    /// Write `content` to `dir/name`, run `patch` on it, return stdout.
    fn run_patch_on(dir: &std::path::Path, name: &str, content: &str, patch_text: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        let cmd = crate::cli::PatchCmd {
            file: path,
            patch: patch_text.to_string(),
            dry_run: false,
            safe: false,
            json: false,
            verbose: false,
            emit_anchors: false,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut ctx = crate::context::CommandContext::new(
            &mut stdout,
            &mut stderr,
            crate::context::OutputMode::Compact,
        );
        super::run(&mut ctx, cmd).expect("patch should succeed");
        String::from_utf8(stdout).unwrap()
    }

    #[test]
    fn patch_preserves_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        run_patch_on(
            dir.path(),
            "bom.txt",
            "\u{FEFF}alpha\nbeta\n",
            "SWAP 2:\n+gamma",
        );
        let disk = std::fs::read_to_string(dir.path().join("bom.txt")).unwrap();
        assert_eq!(disk, "\u{FEFF}alpha\ngamma\n");
    }

    #[test]
    fn patch_preserves_crlf() {
        let dir = tempfile::tempdir().unwrap();
        run_patch_on(dir.path(), "crlf.txt", "a\r\nb\r\n", "SWAP 2:\n+X");
        let disk = std::fs::read(dir.path().join("crlf.txt")).unwrap();
        assert_eq!(disk, b"a\r\nX\r\n");
    }

    #[test]
    fn patch_preserves_pure_cr_and_trailing_terminator() {
        let dir = tempfile::tempdir().unwrap();
        run_patch_on(dir.path(), "cr.txt", "a\rb\r", "SWAP 2:\n+X");
        let disk = std::fs::read(dir.path().join("cr.txt")).unwrap();
        assert_eq!(disk, b"a\rX\r");
    }

    #[test]
    fn patch_pure_cr_without_trailing_terminator() {
        let dir = tempfile::tempdir().unwrap();
        run_patch_on(dir.path(), "cr.txt", "a\rb", "SWAP 2:\n+X");
        let disk = std::fs::read(dir.path().join("cr.txt")).unwrap();
        assert_eq!(disk, b"a\rX");
    }

    #[test]
    fn patch_mixed_eol_canonicalizes_to_dominant() {
        // Documented design choice: mixed-EOL files are rewritten with the
        // dominant ending throughout (per-line endings of untouched lines
        // are NOT preserved — see detect_line_ending docs).
        let dir = tempfile::tempdir().unwrap();
        // CRLF dominant (2) over LF (1): the lone-LF line is rewritten CRLF.
        run_patch_on(dir.path(), "mix.txt", "a\r\nb\r\nc\n", "SWAP 1:\n+A");
        let disk = std::fs::read(dir.path().join("mix.txt")).unwrap();
        assert_eq!(disk, b"A\r\nb\r\nc\r\n");

        // LF dominant (2) over CRLF (1): the CRLF line becomes LF.
        run_patch_on(dir.path(), "mix2.txt", "a\nb\r\nc\n", "SWAP 1:\n+A");
        let disk2 = std::fs::read(dir.path().join("mix2.txt")).unwrap();
        assert_eq!(disk2, b"A\nb\nc\n");
    }

    #[test]
    fn patch_reported_hash_matches_reload_for_cr_file() {
        // The OK line's path#hash must equal what `read`/`FileContent::load`
        // reports after the write — for a pure-CR file this exercises the
        // normalized-hash contract end to end.
        let dir = tempfile::tempdir().unwrap();
        let out = run_patch_on(dir.path(), "h.txt", "x\rb\r", "SWAP 2:\n+c");
        let path = dir.path().join("h.txt");
        // The write-back restored CR endings and kept the trailing terminator.
        assert_eq!(std::fs::read(&path).unwrap(), b"x\rc\r");
        let fc = crate::document::FileContent::load(&path).unwrap();
        assert!(fc.trailing_newline);
        let expected = format!("#{}", fc.hash);
        assert!(
            out.contains(&expected),
            "stdout: {out}\nexpected hash {expected}"
        );
        // And the reported hash is over the LF-normalized content.
        assert_eq!(fc.hash, crate::hash::compute_file_hash("x\nc\n"));
    }

    #[test]
    fn patch_no_trailing_newline_hash_consistency() {
        // Regression: changeset hash must match a fresh load for files
        // without a trailing newline (CR and LF flavours).
        for (name, content, expected_norm) in
            [("lf.txt", "x\nb", "x\nc"), ("cr.txt", "x\rb", "x\nc")]
        {
            let dir = tempfile::tempdir().unwrap();
            let out = run_patch_on(dir.path(), name, content, "SWAP 2:\n+c");
            let fc = crate::document::FileContent::load(&dir.path().join(name)).unwrap();
            assert!(out.contains(&format!("#{}", fc.hash)), "{name}: {out}");
            assert_eq!(fc.hash, crate::hash::compute_file_hash(expected_norm));
        }
    }

    // ---- apply_edits_pure: phantom trailing line (bd-mcp-serve-parity) ----

    #[test]
    fn apply_pure_trailing_blank_line_is_addressable() {
        // `a\n\n` has TWO visible lines — line 2 is a real empty line. The
        // pure path used to build entries without the phantom trailing
        // segment, saw only 1 visible line, and rejected the anchor that
        // the read/patch path accepts — recovery replay then silently
        // returned None.
        let (edits, _w, _f, _a) = parse_patch("DEL 2");
        let r = super::apply_edits_pure("a\n\n", &edits, std::path::Path::new("t.txt"))
            .expect("line 2 must be addressable");
        assert_eq!(r.text, "a\n");
        // The raw-split diff reports the divergence at the phantom third
        // segment; what matters is that a change is reported at all.
        assert!(r.first_changed_line.is_some());
    }

    #[test]
    fn apply_pure_reports_first_changed_line() {
        let (edits, _w, _f, _a) = parse_patch("SWAP 2:\n+x");
        let r =
            super::apply_edits_pure("a\nb\nc\n", &edits, std::path::Path::new("t.txt")).unwrap();
        assert_eq!(r.text, "a\nx\nc\n");
        assert_eq!(r.first_changed_line, Some(2));
    }
}
