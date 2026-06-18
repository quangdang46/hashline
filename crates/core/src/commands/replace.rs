use std::io::Write;

use crate::cli::ReplaceCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::hash::format_short_hash;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReplaceCmd,
) -> Result<(), HashlineError> {
    let fc = FileContent::load(&cmd.file)?;
    let text = &fc.normalized;

    // Find ALL occurrences of old_string in the full normalized content.
    let occurrences: Vec<usize> = text
        .match_indices(&cmd.old_string)
        .map(|(pos, _)| pos)
        .collect();

    if occurrences.is_empty() {
        return Err(HashlineError::QueryNotFound {
            query: cmd.old_string.clone(),
            path: cmd.file.display().to_string(),
        });
    }

    // For ambiguity reporting, determine which lines contain old_string.
    let lines_with: Vec<usize> = {
        let mut line_nos = Vec::new();
        let mut prev_line = 0;
        for &pos in &occurrences {
            let line_no = text[..=pos].matches('\n').count() + 1;
            if line_no != prev_line {
                line_nos.push(line_no);
                prev_line = line_no;
            }
        }
        line_nos
    };

    // Ambiguity: if old_string appears on 2+ distinct lines, error with line numbers.
    // Multiple occurrences on the SAME line are fine — replace the first one.
    if lines_with.len() > 1 {
        let lines_str: Vec<String> = lines_with.iter().map(|l| l.to_string()).collect();
        return Err(HashlineError::AmbiguousQuery {
            query: cmd.old_string.clone(),
            count: occurrences.len(),
            lines: lines_str.join(", "),
            path: cmd.file.display().to_string(),
        });
    }

    // Single match (or multiple on same line) — replace the first occurrence.
    let result_text = text.replacen(&cmd.old_string, &cmd.new_string, 1);
    let changed_line = *lines_with.first().unwrap_or(&1);

    // Compute the new line content and its hash for the JSON response.
    // Find the line containing the match.
    let new_line_content = result_text
        .split('\n')
        .nth(changed_line - 1)
        .unwrap_or("")
        .to_string();
    let new_hash = crate::hash::short_hash_value(&new_line_content);

    // Re-apply line endings.
    let line_ending = crate::normalize::detect_line_ending(&fc.raw);
    let final_text = if line_ending == crate::normalize::LineEnding::Crlf {
        crate::normalize::restore_line_endings(&result_text, line_ending)
    } else {
        result_text
    };

    // Write the result. By default skip fsync for agent speed; --safe does
    // the full atomic-write cycle (temp file + sync_all + rename).
    if cmd.safe {
        crate::commands::common::atomic_write(&cmd.file, final_text.as_bytes())?;
    } else {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&cmd.file)?;
        file.write_all(final_text.as_bytes())?;
    }

    if cmd.json {
        let payload = serde_json::json!({
            "success": true,
            "file": cmd.file.display().to_string(),
            "line": changed_line,
            "new_hash": format_short_hash(new_hash),
            "new_content": new_line_content,
        });
        writeln!(ctx.stdout(), "{}", serde_json::to_string(&payload)?)?;
    }

    Ok(())
}
