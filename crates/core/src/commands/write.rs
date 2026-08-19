use std::io::Write;

use crate::cli::WriteCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;
use crate::hash;
use crate::normalize;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: WriteCmd,
) -> Result<(), HashlineError> {
    // 1. Check file exists — refuse unless --force
    if cmd.file.exists() && !cmd.force {
        return Err(HashlineError::TargetExists {
            path: cmd.file.display().to_string(),
        });
    }

    // 2. Normalize content (LF line endings, strip BOM)
    let normalized = normalize::normalize_to_lf(&cmd.content);
    let bom_result = normalize::strip_bom(&normalized);
    let write_content = bom_result.text;

    // 3. Write using fast_write or atomic_write
    if cmd.safe {
        crate::commands::common::atomic_write(&cmd.file, write_content.as_bytes())?;
    } else {
        crate::commands::common::fast_write(&cmd.file, write_content.as_bytes())?;
    }

    // 4. Compute hash and line count from the written content (canonical path).
    let file_hash = hash::compute_file_hash(&write_content);
    let line_count = write_content.lines().count();

    // 5. Render output based on output mode.
    match ctx.output_mode() {
        crate::context::OutputMode::Compact => {
            // Agent-native: status line only, no re-read
            writeln!(
                ctx.stdout(),
                "OK {}#{} lines={}",
                cmd.file.display(),
                file_hash,
                line_count,
            )?;
        }
        crate::context::OutputMode::Verbose => {
            // Human-readable: re-read and dump full file (old default)
            let fc = crate::document::FileContent::load(&cmd.file)?;
            let raw_lines = fc.lines();
            let entries = fc.lines_with_hashes();
            writeln!(ctx.stdout(), "[{}#{}]", fc.path.display(), fc.hash)?;
            let mut hash_buf = [0u8; 2];
            for (i, entry) in entries.iter().enumerate() {
                if entry.content.is_empty() && i == raw_lines.len() - 1 && fc.trailing_newline {
                    continue;
                }
                hash::write_short_hash_bytes(&mut hash_buf, entry.short_hash);
                let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
                writeln!(ctx.stdout(), "{}:{}|{}", i + 1, hash_str, entry.content)?;
            }
        }
        crate::context::OutputMode::Json => {
            // Structured JSON
            let fc = crate::document::FileContent::load(&cmd.file)?;
            let raw_lines = fc.lines();
            let entries = fc.lines_with_hashes();
            let lines: Vec<serde_json::Value> = entries
                .iter()
                .enumerate()
                .filter(|(i, entry)| {
                    !(entry.content.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline)
                })
                .map(|(i, entry)| {
                    serde_json::json!({
                        "n": i + 1,
                        "hash": hash::format_short_hash(entry.short_hash),
                        "content": entry.content,
                    })
                })
                .collect();
            let output = serde_json::json!({
                "success": true,
                "path": fc.path.display().to_string(),
                "hash": fc.hash,
                "lines": lines,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        crate::context::OutputMode::Ndjson => {
            // Same as compact
            writeln!(
                ctx.stdout(),
                "OK {}#{} lines={}",
                cmd.file.display(),
                file_hash,
                line_count,
            )?;
        }
    }

    Ok(())
}
