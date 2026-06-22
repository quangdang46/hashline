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

    // 4. Re-read and show hashline output
    let fc = crate::document::FileContent::load(&cmd.file)?;
    let raw_lines = fc.lines();
    let entries = fc.lines_with_hashes();

    if cmd.json {
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
    } else {
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

    Ok(())
}
