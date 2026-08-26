use std::io::Write;
use std::path::Path;

use crate::cli::PatchCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::normalize::{LineEnding, detect_line_ending, restore_line_endings};
use crate::parser::parse_patch;
use crate::types::{BlockMode, ChangeSet, ChangedLine, Clipboard, Cursor, Edit, InsertMode};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: PatchCmd,
) -> Result<(), HashlineError> {
    // Resolve patch content: `-` reads stdin, `@path` reads file, otherwise use literal.
    let patch_content = resolve_patch_content(&cmd.patch)?;

    let fc = FileContent::load(&cmd.file)?;
    let text = &fc.normalized;
    let (edits, warnings, _file_op, aborted) = parse_patch(&patch_content);

    // Surface parser warnings even when no edits were produced so callers
    // can tell a syntactically-broken patch from an empty one.
    for w in &warnings {
        eprintln!("warning: {w}");
    }

    if edits.is_empty() {
        if aborted {
            return Ok(());
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

    let entries = fc.lines_with_hashes();
    // Count unique high-level operations (each has a distinct line_num).
    let op_count: usize = edits
        .iter()
        .map(|e| match e {
            Edit::Insert { line_num, .. } => *line_num,
            Edit::Delete { line_num, .. } => *line_num,
            Edit::Block { line_num, .. } => *line_num,
            Edit::Cut { line_num, .. } => *line_num,
            Edit::Paste { line_num, .. } => *line_num,
        })
        .collect::<std::collections::HashSet<_>>()
        .len();
    let changeset = apply_edits_tracked(&mut lines, &entries, &cmd.file, &edits, op_count)?;

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
        result.clone()
    };

    if cmd.dry_run {
        // Use LF-only result for diff comparison so CRLF files don't show
        // spurious \r characters in the output (Bug #101).
        let diff_text = if line_ending == LineEnding::Crlf {
            &result
        } else {
            &final_text
        };
        if cmd.json {
            // Return JSON diff info instead of text diff
            let diff_lines = format_diff(&fc.normalized, diff_text);
            let payload = serde_json::json!({
                "success": true,
                "file": cmd.file.display().to_string(),
                "dry_run": true,
                "edits_applied": edits.len(),
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

    if cmd.safe {
        crate::commands::common::atomic_write(&cmd.file, final_text.as_bytes())?;
    } else {
        crate::commands::common::fast_write(&cmd.file, final_text.as_bytes())?;
    }

    // Render output based on output mode.
    match ctx.output_mode() {
        crate::context::OutputMode::Compact => {
            // Agent-native: OK header + changed lines only
            let path = cmd.file.display();
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
                if content.is_empty() && i == lines.len() - 1 && had_trailing_newline {
                    continue;
                }
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
            let payload = serde_json::json!({
                "success": true,
                "file": cmd.file.display().to_string(),
                "hash": changeset.file_hash,
                "edits_applied": changeset.edits_applied,
                "changed": changed,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&payload)?)?;
        }
        crate::context::OutputMode::Ndjson => {
            // Same as compact for now
            let path = cmd.file.display();
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
/// suitable for dry-run review. Uses a simple LCS-based shortest-edit
/// path that correctly handles insertions and deletions that shift
/// subsequent line indices. Loosely follows `diff -u` style but omits
/// the timestamp header.
fn format_diff(original: &str, final_text: &str) -> Vec<String> {
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
    // Compute LCS table (Wagner-Fischer).
    let m = left.len();
    let n = right.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if left[i - 1] == right[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    // Backtrack to produce edit operations.
    let mut ops: Vec<(usize, &'static str, &str)> = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && left[i - 1] == right[j - 1] {
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push((j, "+", right[j - 1]));
            j -= 1;
        } else {
            ops.push((i, "-", left[i - 1]));
            i -= 1;
        }
    }
    ops.reverse();
    let mut out: Vec<String> = Vec::new();
    out.push("@@ -- ++ @@".into());
    for (_, tag, line) in &ops {
        out.push(format!("{tag}{line}"));
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
    let entries: Vec<crate::document::LineEntry> = lines
        .iter()
        .enumerate()
        .map(|(i, s)| crate::document::LineEntry {
            content: s.clone(),
            short_hash: crate::hash::short_hash_value_indexed(s, i + 1),
        })
        .collect();

    apply_edits(&mut lines, &entries, path, edits)?;

    let result = if text.ends_with('\n') && !lines.is_empty() {
        lines.join("\n") + "\n"
    } else if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    };

    Ok(crate::types::ApplyResult {
        text: result,
        first_changed_line: None,
        warnings: Vec::new(),
        block_resolutions: Vec::new(),
    })
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
pub fn apply_edits_tracked(
    lines: &mut Vec<String>,
    entries: &[crate::document::LineEntry],
    path: &Path,
    edits: &[Edit],
    edits_count: usize,
) -> Result<ChangeSet, HashlineError> {
    apply_edits(lines, entries, path, edits)?;
    let changeset = compute_changeset(entries, lines, edits_count, path);
    Ok(changeset)
}

/// Compute a [`ChangeSet`] by comparing original entries against the final lines.
/// Uses the original entries as the pre-edit snapshot and the final lines as
/// the post-edit result. The `edits_count` is the number of parsed edit operations.
fn compute_changeset(
    entries: &[crate::document::LineEntry],
    final_lines: &[String],
    edits_count: usize,
    _path: &Path,
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

    // Build a map of original content → original line indices.
    let mut orig_content_map: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, entry) in visible_entries.iter().enumerate() {
        orig_content_map
            .entry(entry.content.clone())
            .or_default()
            .push(i);
    }

    // Track which original lines are still present (by content match).
    let mut orig_used: Vec<bool> = vec![false; visible_entries.len()];

    // For each final line, check if it existed in the original.
    for (final_idx, content) in final_lines.iter().enumerate() {
        let line_num = final_idx + 1;
        let hash = crate::hash::format_short_hash(short_hash_value_indexed(content, line_num));

        if let Some(orig_indices) = orig_content_map.get(content) {
            // Find the first unused original occurrence.
            if let Some(&orig_idx) = orig_indices.iter().find(|&&i| !orig_used[i]) {
                orig_used[orig_idx] = true;
                // Line existed and is unchanged — skip.
                continue;
            }
            // Content matches but all occurrences used — treat as insertion.
            inserted.push(ChangedLine {
                line: line_num,
                hash,
                content: content.clone(),
            });
        } else if final_idx < visible_entries.len() {
            // Same position exists in original but content differs — modification.
            orig_used[final_idx] = true; // Mark original as consumed
            modified.push(ChangedLine {
                line: line_num,
                hash,
                content: content.clone(),
            });
        } else {
            // Beyond original bounds — insertion.
            inserted.push(ChangedLine {
                line: line_num,
                hash,
                content: content.clone(),
            });
        }
    }

    // Collect deleted lines — original lines that weren't matched.
    for (i, used) in orig_used.iter().enumerate() {
        if !used {
            deleted.push(i + 1); // 1-indexed
        }
    }

    let final_text = if final_lines.is_empty() {
        String::new()
    } else {
        final_lines.join("\n") + "\n"
    };
    let file_hash = compute_file_hash(&final_text);

    ChangeSet {
        edits_applied: edits_count,
        modified,
        inserted,
        deleted,
        file_hash,
        line_count: final_lines.len(),
    }
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
    use std::collections::HashMap;
    let mut insert_count: HashMap<usize, usize> = HashMap::new();

    // ---- live-position tracking (Bug #89-1) -------------------------------
    let n = entries.len();
    let mut deleted: Vec<bool> = vec![false; n]; // 0‑indexed by original line
    let mut shift: Vec<isize> = vec![0; n]; // cumulative offset per original line

    // Add `delta` to the shift of every original line >= `from` (1‑based).
    fn apply_delta(shift: &mut [isize], from: usize, delta: isize) {
        let start = (from.max(1) - 1).min(shift.len());
        for j in start..shift.len() {
            shift[j] += delta;
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
                        eprintln!("warning: {warning}");
                        replacement_texts = repaired;
                    } else {
                        let issues = crate::apply::detect_boundary_issues(
                            &replacement_texts,
                            start_l,
                            end_l,
                            entries,
                        );
                        for issue in &issues {
                            eprintln!("warning: {issue}");
                        }
                    }
                }

                if num_new > 0 {
                    // ---- live-index adjustment (Bug #89-1) ----
                    let start_idx =
                        if anchor_line >= 1 && anchor_line <= n && !deleted[anchor_line - 1] {
                            let raw = (anchor_line as isize - 1) + shift[anchor_line - 1];
                            if raw >= 0 { Some(raw as usize) } else { None }
                        } else {
                            None
                        }
                        .ok_or_else(|| HashlineError::InvalidAnchor {
                            anchor: format!("line {anchor_line} has been deleted by a prior edit"),
                        })?;
                    let remove_end = (start_idx + num_old).min(lines.len());
                    for _ in start_idx..remove_end {
                        lines.remove(start_idx);
                    }
                    for (k, text) in replacement_texts.iter().enumerate() {
                        lines.insert(start_idx + k, text.clone());
                    }
                }

                // ---- update shift tracking (Bug #89-1) ----
                let first_del = anchor_line;
                let last_del = anchor_line + num_old.saturating_sub(1);
                for dl in first_del..=last_del.min(n) {
                    deleted[dl - 1] = true;
                }
                if last_del < n {
                    apply_delta(
                        &mut shift,
                        last_del + 1,
                        (num_new as isize) - (num_old as isize),
                    );
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
                // Delete from live buffer using shift-adjusted positions
                // Sort DESCENDING so earlier removals don't shift later ones
                del_lines.sort_by(|a, b| b.cmp(a));
                for &orig_line in &del_lines {
                    if orig_line >= 1 && orig_line <= n && !deleted[orig_line - 1] {
                        let raw = (orig_line as isize - 1) + shift[orig_line - 1];
                        let idx = if raw >= 0 { raw as usize } else { continue };
                        if idx < lines.len() {
                            lines.remove(idx);
                        }
                    }
                }

                // ---- update shift tracking (Bug #89-1) ----
                for &dl in &del_lines {
                    if dl <= n {
                        deleted[dl - 1] = true;
                    }
                }
                let last_del = del_lines.iter().max().copied().unwrap_or(0);
                let del_count = del_lines.len();
                if last_del < n && del_count > 0 {
                    apply_delta(&mut shift, last_del + 1, -(del_count as isize));
                }

                i = j;
            }

            // ---- INS.PRE / INS.POST / INS.HEAD / INS.TAIL --------------------
            Edit::Insert { cursor, text, .. } => {
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

                // Anchors address the ORIGINAL snapshot; translate to a live
                // buffer index with the same shift table SWAP/DEL/CUT/PUT use.
                let live = |orig: usize| -> usize {
                    if orig >= 1 && orig <= n {
                        let raw = (orig as isize - 1) + shift[orig - 1];
                        if raw >= 0 { raw as usize } else { 0 }
                    } else {
                        orig.saturating_sub(1)
                    }
                };
                let (base_line, stack_key) = match cursor {
                    // INS.PRE bumps shift at a.line inclusive, so repeated
                    // PREs at one anchor already stack via `live`.
                    Cursor::BeforeAnchor(a) => (live(a.line), None),
                    // INS.POST bumps shift only from a.line+1 (so a SWAP of
                    // a.line still hits the original line), hence the counter.
                    Cursor::AfterAnchor(a) => (live(a.line) + 1, Some(a.line)),
                    Cursor::Bof => (0, Some(0)),
                    Cursor::Eof => (lines.len(), None),
                };

                let offset = match stack_key {
                    Some(key) => {
                        let o = insert_count.get(&key).copied().unwrap_or(0);
                        insert_count.insert(key, o + 1);
                        o
                    }
                    None => 0,
                };
                let pos = if base_line + offset <= lines.len() {
                    base_line + offset
                } else {
                    lines.len()
                };
                lines.insert(pos, text.clone());

                // ---- update shift tracking (Bug #89-1) ----
                match cursor {
                    Cursor::BeforeAnchor(a) => {
                        // Insert before original line N → lines at N+ shift by +1
                        if a.line <= n {
                            apply_delta(&mut shift, a.line, 1);
                        }
                    }
                    Cursor::AfterAnchor(a) => {
                        // Insert after original line N → lines > N shift by +1
                        if a.line < n {
                            apply_delta(&mut shift, a.line + 1, 1);
                        }
                    }
                    Cursor::Bof => {
                        if n > 0 {
                            apply_delta(&mut shift, 1, 1);
                        }
                    }
                    Cursor::Eof => {
                        // Insert at end → no change to any original line's live position
                    }
                }

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

                // Delete from the live buffer using shift-adjusted positions
                // (same as DEL: sort descending so earlier removals don't
                // shift later ones).
                let mut del_lines: Vec<usize> = (start_line..=end_line).collect();
                del_lines.sort_by(|a, b| b.cmp(a));
                for &orig_line in &del_lines {
                    if orig_line >= 1 && orig_line <= n && !deleted[orig_line - 1] {
                        let raw = (orig_line as isize - 1) + shift[orig_line - 1];
                        let idx = if raw >= 0 { raw as usize } else { continue };
                        if idx < lines.len() {
                            lines.remove(idx);
                        }
                    }
                }

                // ---- update shift tracking (Bug #89-1) ----
                for &dl in &del_lines {
                    if dl <= n {
                        deleted[dl - 1] = true;
                    }
                }
                let last_del = del_lines.iter().max().copied().unwrap_or(0);
                let del_count = del_lines.len();
                if last_del < n && del_count > 0 {
                    apply_delta(&mut shift, last_del + 1, -(del_count as isize));
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
                let num_lines = captured.len();

                // Resolve the live insertion index, shift-adjusted so a CUT
                // earlier in the same patch is accounted for.
                let pos = match cursor {
                    Cursor::BeforeAnchor(a) => {
                        if a.line > visible_lines {
                            return Err(HashlineError::InvalidAnchor {
                                anchor: format!(
                                    "line {} not found (file has {visible_lines} lines)",
                                    a.line,
                                ),
                            });
                        }
                        if a.line >= 1 && a.line <= n && !deleted[a.line - 1] {
                            let raw = (a.line as isize - 1) + shift[a.line - 1];
                            if raw >= 0 { raw as usize } else { 0 }
                        } else {
                            a.line.wrapping_sub(1)
                        }
                    }
                    Cursor::Bof => 0,
                    _ => {
                        return Err(HashlineError::InvalidAnchor {
                            anchor: "PUT target must be before a line".to_string(),
                        });
                    }
                };

                let offset = insert_count.get(&pos).copied().unwrap_or(0);
                insert_count.insert(pos, offset + num_lines);
                let insert_pos = if pos + offset <= lines.len() {
                    pos + offset
                } else {
                    lines.len()
                };
                for (k, text) in captured.iter().enumerate() {
                    lines.insert(insert_pos + k, text.clone());
                }

                // ---- update shift tracking (Bug #89-1) ----
                match cursor {
                    Cursor::BeforeAnchor(a) => {
                        if a.line <= n {
                            apply_delta(&mut shift, a.line, num_lines as isize);
                        }
                    }
                    Cursor::Bof if n > 0 => {
                        apply_delta(&mut shift, 1, num_lines as isize);
                    }
                    _ => {}
                }

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

                // Resolve the syntactic block starting at line_no.
                // NOTE: this resolves against the ORIGINAL snapshot. After
                // prior edits in the same patch remove/insert lines, the
                // original line numbers may no longer correspond to the
                // intended blocks. This is a known limitation of block ops
                // in multi-edit patches.
                let (orig_block_start, orig_block_end) =
                    resolve_block_span(entries, anchor_index, path)?;

                // Translate block boundaries to live buffer indices through the
                // same shift table used by SWAP/DEL/CUT/PUT/INS (Bug #106).
                // Only translate block_start; derive block_end from it to
                // preserve the original block span.
                let block_start = {
                    let raw = (orig_block_start as isize) + shift[orig_block_start.min(n - 1)];
                    raw.max(0) as usize
                };
                let block_len = orig_block_end - orig_block_start + 1;
                let block_end = block_start + block_len - 1;

                match mode {
                    None if payloads.is_empty() => {
                        // DEL.BLK N — remove translated range in descending
                        // order so earlier removals don't shift later indices.
                        let del_end = block_end.min(lines.len().saturating_sub(1));
                        for idx in (block_start..=del_end).rev() {
                            if idx < lines.len() {
                                lines.remove(idx);
                            }
                        }
                        // Consume trailing blank line after the deleted block.
                        // Track whether one was consumed so shift is updated.
                        let consumed_blank =
                            block_start < lines.len() && lines[block_start].trim().is_empty();
                        if consumed_blank {
                            lines.remove(block_start);
                        }

                        // ---- update shift tracking (uses original indices) ----
                        for dl in (orig_block_start + 1)..=(orig_block_end + 1).min(n) {
                            deleted[dl - 1] = true;
                        }
                        // Account for both the block lines AND any consumed
                        // trailing blank in the shift delta so subsequent block
                        // ops translate against correct live positions.
                        let total_removed = block_len + if consumed_blank { 1 } else { 0 };
                        if orig_block_end + 1 < n {
                            apply_delta(&mut shift, orig_block_end + 2, -(total_removed as isize));
                        }
                    }
                    None => {
                        // SWAP.BLK N: replace the entire block with payload
                        let num_old = block_end - block_start + 1;
                        for _ in 0..num_old.min(lines.len()) {
                            if block_start < lines.len() {
                                lines.remove(block_start);
                            }
                        }
                        for (k, payload) in payloads.iter().enumerate() {
                            lines.insert(block_start + k, payload.clone());
                        }

                        // ---- update shift tracking (uses original indices) ----
                        for dl in (orig_block_start + 1)..=(orig_block_end + 1).min(n) {
                            deleted[dl - 1] = true;
                        }
                        if orig_block_end + 1 < n {
                            apply_delta(
                                &mut shift,
                                orig_block_end + 2,
                                (payloads.len() as isize) - (num_old as isize),
                            );
                        }
                    }
                    Some(BlockMode::InsertAfter) => {
                        // INS.BLK.POST N: insert after the last line of the block
                        let insert_pos = (block_end + 1).min(lines.len());
                        for (k, payload) in payloads.iter().enumerate() {
                            lines.insert(insert_pos + k, payload.clone());
                        }

                        // ---- update shift tracking (uses original indices) ----
                        if orig_block_end + 1 < n {
                            apply_delta(&mut shift, orig_block_end + 2, payloads.len() as isize);
                        }
                    }
                    Some(BlockMode::InsertBefore) => {
                        // INS.BLK.PRE N: insert before the first line of the block
                        let insert_pos = block_start.min(lines.len());
                        for (k, payload) in payloads.iter().enumerate() {
                            lines.insert(insert_pos + k, payload.clone());
                        }

                        // ---- update shift tracking (uses original indices) ----
                        if orig_block_start < n {
                            apply_delta(&mut shift, orig_block_start + 1, payloads.len() as isize);
                        }
                    }
                }

                i += 1;
            }
        }
    }
    Ok(())
}

/// Resolve a 1-indexed anchor line to the syntactic block span (0-based inclusive).
///
/// Uses language detection from file extension, then brace-matching,
/// indentation-based, or Ruby `end`-based block finding.
fn resolve_block_span(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    path: &Path,
) -> Result<(usize, usize), HashlineError> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    match extension {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp" | "cs"
        | "kt" | "kts" | "swift" | "scala" | "dart" | "zig" | "m" | "mm" => {
            find_brace_block(entries, anchor_index, extension)
        }
        "py" | "verse" => find_python_block(entries, anchor_index),
        "rb" => find_ruby_block(entries, anchor_index),
        _ => find_brace_block(entries, anchor_index, extension)
            .or_else(|_| find_indent_block(entries, anchor_index)),
    }
    .map_err(|_| HashlineError::UnbalancedBlock {
        line_no: anchor_index + 1,
    })
}

// ---------------------------------------------------------------------------
// Brace-balanced block finding
// ---------------------------------------------------------------------------

fn find_brace_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    ext: &str,
) -> Result<(usize, usize), ()> {
    let pairs = find_brace_pairs(entries, ext);
    pairs
        .iter()
        .filter(|(start, end)| *start <= anchor_index && *end >= anchor_index)
        .max_by_key(|(start, _)| *start)
        .copied()
        .ok_or(())
}

fn find_brace_pairs(entries: &[crate::document::LineEntry], _ext: &str) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let line_comment: &[u8] = b"//";
    let mut in_block_comment = false;

    for (line_idx, entry) in entries.iter().enumerate() {
        let bytes = entry.content.as_bytes();
        let mut i = 0;
        let mut in_sq = false;
        let mut in_dq = false;
        let mut esc = false;

        while i < bytes.len() {
            if esc {
                esc = false;
                i += 1;
                continue;
            }
            if (in_sq || in_dq) && bytes[i] == b'\\' {
                esc = true;
                i += 1;
                continue;
            }
            if in_block_comment {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i..].starts_with(line_comment) {
                break;
            }
            if !in_sq && !in_dq && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                in_block_comment = true;
                i += 2;
                continue;
            }
            if in_sq && bytes[i] == b'\'' {
                in_sq = false;
                i += 1;
                continue;
            }
            if in_dq && bytes[i] == b'"' {
                in_dq = false;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i] == b'\'' {
                in_sq = true;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && bytes[i] == b'"' {
                in_dq = true;
                i += 1;
                continue;
            }
            if !in_sq && !in_dq && !in_block_comment {
                if bytes[i] == b'{' {
                    stack.push(line_idx);
                } else if bytes[i] == b'}' {
                    if let Some(s) = stack.pop() {
                        pairs.push((s, line_idx));
                    }
                }
            }
            i += 1;
        }
    }
    pairs
}

// ---------------------------------------------------------------------------
// Indentation-based block finding
// ---------------------------------------------------------------------------

fn find_indent_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    // If anchor IS at indent 0, it's a block header itself.
    if anchor_indent == 0 {
        find_block_from_header(entries, anchor_index, &["//"])
    } else {
        find_block_from_body(entries, anchor_index, &["//", "#"])
    }
}

fn find_python_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    if anchor_indent == 0 {
        // Anchor at a header line (def/class/if/for/with/try/…):
        // find the block starting at that header to its dedent boundary.
        find_block_from_header(entries, anchor_index, &["#"])
    } else {
        // Anchor in a body line (comment, bare statement, etc):
        // start at the anchor itself and extend to the next line with the
        // same or lesser indent level.  This gives the smallest enclosing
        // statement suite, not the outermost function/class.
        let mut end = entries.len() - 1;
        for i in (anchor_index + 1)..entries.len() {
            let t = entries[i].content.trim();
            if t.is_empty() {
                continue;
            }
            if leading_ws(&entries[i].content) <= anchor_indent {
                end = i.saturating_sub(1);
                break;
            }
        }
        Ok((anchor_index, end))
    }
}

/// Block header line (indent 0): scan forward to find same-or-less indent.
fn find_block_from_header(
    entries: &[crate::document::LineEntry],
    start: usize,
    _comments: &[&str],
) -> Result<(usize, usize), ()> {
    let si = leading_ws(&entries[start].content);
    let mut end = entries.len() - 1;
    for i in (start + 1)..entries.len() {
        if leading_ws(&entries[i].content) <= si {
            end = i.saturating_sub(1);
            break;
        }
    }
    Ok((start, end))
}

/// Block body line (indented): scan backward for header, then forward for end.
fn find_block_from_body(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    _comments: &[&str],
) -> Result<(usize, usize), ()> {
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    let mut start = None;
    for i in (0..anchor_index).rev() {
        if entries[i].content.trim().is_empty() {
            continue;
        }
        if leading_ws(&entries[i].content) < anchor_indent {
            start = Some(i);
            break;
        }
    }
    let start = start.ok_or(())?;
    let si = leading_ws(&entries[start].content);
    let mut end = entries.len() - 1;
    for i in (start + 1)..entries.len() {
        let t = entries[i].content.trim();
        if t.is_empty() {
            continue;
        }
        if leading_ws(&entries[i].content) <= si {
            end = i.saturating_sub(1);
            break;
        }
    }
    Ok((start, end))
}

// ---------------------------------------------------------------------------
// Ruby ...end block finding
// ---------------------------------------------------------------------------

const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ",
];

fn find_ruby_block(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
) -> Result<(usize, usize), ()> {
    let mut depth: isize = 0;
    let mut start = None;
    for i in (0..=anchor_index).rev() {
        let trimmed = entries[i].content.trim();
        let ec = if trimmed == "end" { 1 } else { 0 };
        let oc = ruby_opener_count(trimmed);
        depth += ec as isize;
        depth -= oc as isize;
        if oc > 0 && depth <= 0 {
            start = Some(i);
            break;
        }
    }
    let start = start.ok_or(())?;
    depth = 0;
    for i in start..entries.len() {
        let trimmed = entries[i].content.trim();
        let oc = ruby_opener_count(trimmed);
        let ec = if trimmed == "end" { 1 } else { 0 };
        depth += oc as isize;
        depth -= ec as isize;
        if i > start && depth <= 0 && trimmed == "end" {
            return Ok((start, i));
        }
        if i == start && depth <= 0 {
            return Ok((start, i));
        }
    }
    Err(())
}

fn ruby_opener_count(trimmed: &str) -> usize {
    for opener in RUBY_OPENERS {
        if trimmed.starts_with(opener) {
            return 1;
        }
    }
    0
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start().len()
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
}
