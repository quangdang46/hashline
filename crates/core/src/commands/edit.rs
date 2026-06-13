use std::io::Write;

use crate::anchor::{
    looks_like_range_anchor, parse_anchor, parse_range, resolve, resolve_query_region, resolve_range,
};
use crate::cli::EditCmd;
use memmap2::Mmap;

use crate::commands::common::{
    atomic_write, atomic_write_document, atomic_write_with, check_guard, interpret_escapes,
};
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash_cache::discover_sidecar_root;
use crate::mutation::{replace_line, replace_range, split_content_lines, stream_replace_line};
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: EditCmd,
) -> Result<(), HashlineError> {
    if cmd.streaming {
        return run_streaming(ctx, cmd);
    }

    let root = discover_sidecar_root(&cmd.file);
    let mut doc = Document::load_with_hash_cache(&cmd.file, &root)?;
    check_guard(&doc, cmd.expect_mtime, cmd.expect_inode)?;
    let needs_receipt = cmd.receipt || cmd.audit_log.is_some();
    let before_bytes = needs_receipt.then(|| doc.render());

    let content = if cmd.interpret_escapes {
        interpret_escapes(&cmd.content)
    } else {
        cmd.content
    };

    let index = doc.build_index();
    let summary = if let Some(_) = cmd.start_query {
        let region = resolve_query_region(
            &doc,
            cmd.start_query.as_deref(),
            cmd.end_query.as_deref(),
        )?
        .expect("start_query is set so region is Some");
        let start_idx = region.start_line - 1;
        let end_idx = region.end_line - 1;
        let before = doc.lines[start_idx..=end_idx]
            .iter()
            .map(|line| line.content.to_string())
            .collect::<Vec<_>>();
        let after = split_content_lines(&content);
        replace_range(&mut doc, start_idx, end_idx, &content)?;
        EditSummary::Range {
            start_line: region.start_line,
            end_line: region.end_line,
            before,
            after: after.iter().map(|s| s.to_string()).collect(),
        }
    } else {
        match looks_like_range_anchor(&cmd.anchor) {
            true => {
                let range = parse_range(&cmd.anchor)?;
                let (start, end) = resolve_range(&range, &doc, &index)?;
                let before = doc.lines[start.index..=end.index]
                    .iter()
                    .map(|line| line.content.to_string())
                    .collect::<Vec<_>>();
                let after = split_content_lines(&content);
                replace_range(&mut doc, start.index, end.index, &content)?;
                EditSummary::Range {
                    start_line: start.line_no,
                    end_line: end.line_no,
                    before,
                    after: after.iter().map(|s| s.to_string()).collect(),
                }
            }
            false => {
                let anchor = parse_anchor(&cmd.anchor)?;
                let resolved = resolve(&anchor, &doc, &index)?;
                let before = doc.lines[resolved.index].content.to_string();
                if content.contains(['\n', '\r']) {
                    // Replacement content spans multiple lines; treat the single
                    // anchor as a one-line range so callers can expand a single
                    // line into many without explicitly writing `H..H`.
                    let after_lines = split_content_lines(&content);
                    replace_range(&mut doc, resolved.index, resolved.index, &content)?;
                    EditSummary::Range {
                        start_line: resolved.line_no,
                        end_line: resolved.line_no,
                        before: vec![before],
                        after: after_lines.iter().map(|s| s.to_string()).collect(),
                    }
                } else {
                    replace_line(&mut doc, resolved.index, &content)?;
                    EditSummary::Single {
                        line_no: resolved.line_no,
                        before,
                        after: content,
                    }
                }
            }
        }
    };

    if cmd.dry_run {
        return write_dry_run(ctx, &cmd.file, &summary);
    }

    let after_bytes = if needs_receipt {
        let bytes = doc.render();
        atomic_write(&cmd.file, &bytes)?;
        Some(bytes)
    } else if let EditSummary::Single {
        line_no,
        before,
        after,
    } = &summary
    {
        if !atomic_write_single_line_edit(&cmd.file, &doc, line_no - 1, before, after)? {
            atomic_write_document(&cmd.file, &doc)?;
        }
        None
    } else {
        atomic_write_document(&cmd.file, &doc)?;
        None
    };

    // Seed the session cache with the post-mutation document so the
    // MCP server (or any caller) can avoid a disk re-read.
    ctx.modified_doc = Some(doc.clone());

    if needs_receipt {
        let before_bytes = before_bytes.as_deref().ok_or_else(|| {
            std::io::Error::other("before bytes should exist when receipt is needed")
        })?;
        let after_bytes = after_bytes.as_deref().ok_or_else(|| {
            std::io::Error::other("after bytes should exist when receipt is needed")
        })?;
        let receipt = receipt::build_receipt(
            "edit",
            &cmd.file,
            summary.line_changes(),
            before_bytes,
            after_bytes,
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
            output::write_success_line(ctx, &summary.success_message())
                .map_err(HashlineError::from)?;
            // Phase 2.3: emit fresh anchors of the changed region so the
            // agent doesn't need a follow-up `read` call to verify the edit
            // or anchor a subsequent edit.
            let (first, last) = match &summary {
                EditSummary::Single { line_no, .. } => (*line_no, *line_no),
                EditSummary::Range {
                    start_line, after, ..
                } => {
                    let last = start_line + after.len().saturating_sub(1);
                    (*start_line, last.max(*start_line))
                }
            };
            output::write_post_edit_snippet(ctx, &doc, first, last).map_err(HashlineError::from)?;
            Ok(())
        }
    }
}

/// Streaming edit path: reads the file line-by-line with BufReader instead
/// of loading the full Document into memory. Requires a qualified anchor
/// (line:hash) and single-line content.
///
/// No post-mutation cache is populated since the full document was never
/// loaded. This saves significant memory on files over 100k lines.
fn run_streaming<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: EditCmd,
) -> Result<(), HashlineError> {
    // Streaming mode only supports qualified line:hash anchors (not raw hashes or ranges).
    if looks_like_range_anchor(&cmd.anchor) {
        return Err(HashlineError::InvalidAnchor {
            anchor: cmd.anchor,
        });
    }

    let anchor = parse_anchor(&cmd.anchor)?;
    let (line_no, short) = match anchor {
        crate::anchor::Anchor::LineHash { line, short } => (line, short),
        _ => {
            return Err(HashlineError::InvalidAnchor {
                anchor: cmd.anchor,
            })
        }
    };
    // line_no is 1-indexed; convert to 0-indexed.
    let target_line = line_no.checked_sub(1).ok_or_else(|| {
        HashlineError::InvalidAnchor {
            anchor: cmd.anchor.clone(),
        }
    })?;

    let content = if cmd.interpret_escapes {
        interpret_escapes(&cmd.content)
    } else {
        cmd.content
    };

    // Streaming mode requires single-line content.
    if content.contains(['\n', '\r']) {
        return Err(HashlineError::MultiLineContentUnsupported);
    }

    // Determine newline style and trailing-newline flag via a lightweight
    // streaming scan that avoids loading the full file.
    let streaming_doc = crate::document::StreamingDocument::scan(&cmd.file)?;

    if cmd.dry_run {
        let summary = EditSummary::Single {
            line_no,
            before: "(streaming)".to_owned(),
            after: content.clone(),
        };
        return write_dry_run(ctx, &cmd.file, &summary);
    }

    stream_replace_line(
        &cmd.file,
        target_line,
        &content,
        short,
        streaming_doc.newline,
        streaming_doc.trailing_newline,
    )?;

    // No post-mutation cache since the Document was never loaded.
    // ctx.modified_doc remains None, matching the trade-off of memory
    // over convenience.

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => Ok(()),
        OutputMode::Pretty => {
            output::write_success_line(ctx, &format!("Edited line {line_no}."))
                .map_err(HashlineError::from)
        }
    }
}

fn atomic_write_single_line_edit(
    path: &std::path::Path,
    doc: &Document,
    line_index: usize,
    before: &str,
    after: &str,
) -> Result<bool, HashlineError> {
    let file = std::fs::File::open(path)?;
    let mmap = unsafe { Mmap::map(&file) }?;
    let Some((start, end)) = original_line_byte_span(doc, line_index, before.len()) else {
        return Ok(false);
    };

    if end > mmap.len() || &mmap[start..end] != before.as_bytes() {
        return Ok(false);
    }

    #[cfg(windows)]
    {
        // On Windows, an active mmap prevents file rename (os error 1224).
        // Copy the byte ranges out before the mmap is dropped.
        let head = mmap[..start].to_vec();
        let tail = mmap[end..].to_vec();
        drop(mmap);
        drop(file);

        atomic_write_with(path, |writer| {
            writer.write_all(&head)?;
            writer.write_all(after.as_bytes())?;
            writer.write_all(&tail)?;
            Ok(())
        })?;
        Ok(true)
    }

    #[cfg(not(windows))]
    {
        // On Unix the source file can stay mmap'd during the temp-file write
        // and the subsequent rename, so we skip the two `to_vec()` copies of
        // the unchanged head/tail (which dominate `edit` latency on large
        // files — 5.9 MB of unnecessary user-space copy on a 100k-line file).
        let head: &[u8] = &mmap[..start];
        let tail: &[u8] = &mmap[end..];
        atomic_write_with(path, |writer| {
            writer.write_all(head)?;
            writer.write_all(after.as_bytes())?;
            writer.write_all(tail)?;
            Ok(())
        })?;
        drop(mmap);
        drop(file);
        Ok(true)
    }
}

fn original_line_byte_span(
    doc: &Document,
    line_index: usize,
    original_line_len: usize,
) -> Option<(usize, usize)> {
    if line_index >= doc.lines.len() {
        return None;
    }

    let separator_len = match doc.newline {
        crate::document::NewlineStyle::Lf => 1,
        crate::document::NewlineStyle::Crlf => 2,
    };
    let start = doc.lines[..line_index]
        .iter()
        .map(|line| line.content.len() + separator_len)
        .sum::<usize>();

    Some((start, start + original_line_len))
}

fn write_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    file: &std::path::Path,
    summary: &EditSummary,
) -> Result<(), HashlineError> {
    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            // PR-D: emit a compact mutation receipt instead of dumping the
            // entire proposed document.
            let receipt = receipt::build_dry_run_receipt(
                "edit",
                file,
                summary.dry_run_summary(),
                summary.line_changes(),
            );
            receipt::write_dry_run_receipt(ctx, &receipt)
        }
        OutputMode::Pretty => {
            match summary {
                EditSummary::Single {
                    line_no,
                    before,
                    after,
                } => {
                    output::write_success_line(ctx, &format!("Would change line {line_no}:"))?;
                    output::write_success_line(ctx, &format!("  - {before:?}"))?;
                    output::write_success_line(ctx, &format!("  + {after:?}"))?;
                }
                EditSummary::Range {
                    start_line,
                    end_line,
                    before,
                    after,
                } => {
                    output::write_success_line(
                        ctx,
                        &format!("Would change lines {start_line}-{end_line}:"),
                    )?;
                    for line in before {
                        output::write_success_line(ctx, &format!("  - {line:?}"))?;
                    }
                    for line in after {
                        output::write_success_line(ctx, &format!("  + {line:?}"))?;
                    }
                }
            }
            output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)
        }
    }
}

enum EditSummary {
    Single {
        line_no: usize,
        before: String,
        after: String,
    },
    Range {
        start_line: usize,
        end_line: usize,
        before: Vec<String>,
        after: Vec<String>,
    },
}

impl EditSummary {
    fn success_message(&self) -> String {
        match self {
            EditSummary::Single { line_no, .. } => format!("Edited line {line_no}."),
            EditSummary::Range {
                start_line,
                end_line,
                ..
            } => format!("Edited lines {start_line}-{end_line}."),
        }
    }

    fn dry_run_summary(&self) -> String {
        match self {
            EditSummary::Single { line_no, .. } => format!("Would edit line {line_no}."),
            EditSummary::Range {
                start_line,
                end_line,
                ..
            } => format!("Would edit lines {start_line}-{end_line}."),
        }
    }

    fn line_changes(&self) -> Vec<LineChange> {
        match self {
            EditSummary::Single {
                line_no,
                before,
                after,
            } => vec![LineChange {
                line_no: *line_no,
                kind: ChangeKind::Modified,
                before: Some(before.clone()),
                after: Some(after.clone()),
            }],
            EditSummary::Range {
                start_line,
                before,
                after,
                ..
            } => {
                let shared = before.len().min(after.len());
                let mut changes = Vec::with_capacity(before.len().max(after.len()));

                for index in 0..shared {
                    changes.push(LineChange {
                        line_no: *start_line + index,
                        kind: ChangeKind::Modified,
                        before: Some(before[index].clone()),
                        after: Some(after[index].clone()),
                    });
                }

                for (offset, removed) in before.iter().enumerate().skip(shared) {
                    changes.push(LineChange {
                        line_no: *start_line + offset,
                        kind: ChangeKind::Deleted,
                        before: Some(removed.clone()),
                        after: None,
                    });
                }

                for (offset, inserted) in after.iter().enumerate().skip(shared) {
                    changes.push(LineChange {
                        line_no: *start_line + offset,
                        kind: ChangeKind::Inserted,
                        before: None,
                        after: Some(inserted.clone()),
                    });
                }

                changes
            }
        }
    }
}
