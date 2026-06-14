use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use regex::Regex;
use tempfile::NamedTempFile;

use crate::context::{CommandContext, OutputMode};
use crate::error::HashlineError;
use crate::output;

/// Stream-based text replacement.
///
/// For single-line patterns (no `\n` in old_text), uses a streaming approach
/// (BufReader → temp file → atomic rename) to avoid loading the full file
/// into memory. For multi-line patterns, falls back to loading the full content.
///
/// Like str_replace: no anchors, no hashes, just find-and-replace with
/// atomic write safety.
pub fn stream_replace_text(
    path: &Path,
    old_text: &str,
    new_text: &str,
    max_count: usize,
    use_regex: bool,
) -> Result<ReplaceReceipt, HashlineError> {
    let start = std::time::Instant::now();

    if use_regex || old_text.contains('\n') {
        // Regex or multi-line pattern: need full content in memory
        replace_full(path, old_text, new_text, max_count, use_regex, start)
    } else {
        // Single-line: use streaming BufReader
        replace_streaming(path, old_text, new_text, max_count, start)
    }
}

/// Streaming path: single-line replacement via BufReader.
/// Only loads one line at a time. O(1) memory relative to file size.
fn replace_streaming(
    path: &Path,
    old: &str,
    new: &str,
    max_count: usize,
    start: std::time::Instant,
) -> Result<ReplaceReceipt, HashlineError> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);

    let parent = path.parent().unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(meta.permissions());
    }

    let mut replacements = 0usize;
    let mut remaining = if max_count == 0 {
        usize::MAX
    } else {
        max_count
    };

    for line_result in reader.lines() {
        let line = line_result?;
        if remaining > 0 && line.contains(old) {
            let replaced = line.replace(old, new);
            writeln!(temp, "{replaced}")?;
            replacements += 1;
            remaining -= 1;
        } else {
            writeln!(temp, "{line}")?;
        }
    }

    if replacements == 0 {
        return Ok(ReplaceReceipt {
            matched: false,
            replacements: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    temp.persist(path)
        .map_err(|e| HashlineError::Io(std::io::Error::other(e.to_string())))?;

    Ok(ReplaceReceipt {
        matched: true,
        replacements,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Full-content path: reads entire file into memory, applies replacement,
/// writes atomically. Used for regex and multi-line patterns.
fn replace_full(
    path: &Path,
    old_text: &str,
    new_text: &str,
    max_count: usize,
    use_regex: bool,
    start: std::time::Instant,
) -> Result<ReplaceReceipt, HashlineError> {
    let file = std::fs::File::open(path)?;
    let mut content = String::new();
    BufReader::new(file).read_to_string(&mut content)?;

    let (result, actual_count) = if use_regex {
        replace_regex(&content, old_text, new_text, max_count)?
    } else {
        replace_plain(&content, old_text, new_text, max_count)
    };

    if actual_count == 0 {
        return Ok(ReplaceReceipt {
            matched: false,
            replacements: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    let parent = path.parent().unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    if let Ok(meta) = std::fs::metadata(path) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    temp.write_all(result.as_bytes())?;
    temp.persist(path)
        .map_err(|e| HashlineError::Io(std::io::Error::other(e.to_string())))?;

    Ok(ReplaceReceipt {
        matched: true,
        replacements: actual_count,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

fn replace_plain(content: &str, old: &str, new: &str, max_count: usize) -> (String, usize) {
    if old.is_empty() {
        return (content.to_owned(), 0);
    }
    if max_count == 0 {
        let count = count_occurrences(content, old);
        (content.replace(old, new), count)
    } else {
        let mut result = String::with_capacity(content.len());
        let mut remaining = max_count;
        let mut pos = 0;
        while remaining > 0 {
            if let Some(idx) = content[pos..].find(old) {
                result.push_str(&content[pos..pos + idx]);
                result.push_str(new);
                pos += idx + old.len();
                remaining -= 1;
            } else {
                break;
            }
        }
        result.push_str(&content[pos..]);
        (result, max_count - remaining)
    }
}

fn replace_regex(
    content: &str,
    pattern: &str,
    new: &str,
    max_count: usize,
) -> Result<(String, usize), HashlineError> {
    let re = Regex::new(pattern).map_err(|e| HashlineError::InvalidAnchor {
        anchor: format!("invalid regex: {e}"),
    })?;

    let count = re.find_iter(content).count();
    if count == 0 {
        return Ok((content.to_owned(), 0));
    }

    let actual = if max_count == 0 {
        count
    } else {
        max_count.min(count)
    };
    let result = if max_count == 0 || max_count >= count {
        re.replace_all(content, new).to_string()
    } else {
        let mut result = String::with_capacity(content.len());
        let mut last_end = 0;
        let mut remaining = max_count;
        for m in re.find_iter(content) {
            result.push_str(&content[last_end..m.start()]);
            result.push_str(new);
            last_end = m.end();
            remaining -= 1;
            if remaining == 0 {
                result.push_str(&content[last_end..]);
                break;
            }
        }
        result
    };

    Ok((result, actual))
}

fn count_occurrences(content: &str, pattern: &str) -> usize {
    let mut count = 0;
    let mut pos = 0;
    while let Some(idx) = content[pos..].find(pattern) {
        count += 1;
        pos += idx + pattern.len();
    }
    count
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceReceipt {
    pub matched: bool,
    pub replacements: usize,
    pub duration_ms: u64,
}

/// CLI entry point.
pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: crate::cli::ReplaceCmd,
) -> Result<(), HashlineError> {
    if cmd.dry_run {
        return run_dry_run(ctx, cmd);
    }

    let receipt = stream_replace_text(
        &cmd.file,
        &cmd.old_text,
        &cmd.new_text,
        cmd.count,
        cmd.regex,
    )?;

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            let json = serde_json::to_string(&receipt).map_err(HashlineError::from)?;
            writeln!(ctx.stdout(), "{json}").map_err(HashlineError::from)?;
        }
        OutputMode::Pretty => {
            if receipt.matched {
                output::write_success_line(
                    ctx,
                    &format!(
                        "Replaced {} occurrence(s) in {} ms.",
                        receipt.replacements, receipt.duration_ms
                    ),
                )
                .map_err(HashlineError::from)?;
            } else {
                output::write_success_line(ctx, "No matches found.")
                    .map_err(HashlineError::from)?;
            }
        }
    }
    Ok(())
}

fn run_dry_run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: crate::cli::ReplaceCmd,
) -> Result<(), HashlineError> {
    let file = std::fs::File::open(&cmd.file)?;
    let mut content = String::new();
    BufReader::new(file).read_to_string(&mut content)?;

    let count = if cmd.regex {
        let re = Regex::new(&cmd.old_text).map_err(|e| HashlineError::InvalidAnchor {
            anchor: format!("invalid regex: {e}"),
        })?;
        re.find_iter(&content).count()
    } else if cmd.old_text.is_empty() {
        0
    } else {
        content.matches(&cmd.old_text).count()
    };

    match ctx.output_mode() {
        OutputMode::Json | OutputMode::Ndjson => {
            writeln!(
                ctx.stdout(),
                "{}",
                serde_json::json!({"matches": count, "dry_run": true})
            )
            .map_err(HashlineError::from)?;
        }
        OutputMode::Pretty => {
            output::write_success_line(
                ctx,
                &format!("Dry-run: {count} match(es) found. Use without --dry-run to replace."),
            )
            .map_err(HashlineError::from)?;
        }
    }
    Ok(())
}
