use std::io::Write;

use crate::cli::ReadCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::hash::{format_short_hash, write_short_hash_bytes};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReadCmd,
) -> Result<(), HashlineError> {
    let fc = FileContent::load(&cmd.file)?;
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
                    "hash": format_short_hash(entry.short_hash),
                    "content": entry.content,
                })
            })
            .collect();
        let output = serde_json::json!({
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
            write_short_hash_bytes(&mut hash_buf, entry.short_hash);
            let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
            writeln!(ctx.stdout(), "{}:{}|{}", i + 1, hash_str, entry.content)?;
        }
    }

    Ok(())
}
