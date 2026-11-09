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
        // Stream the JSON document piecewise instead of materializing a
        // Vec<serde_json::Value> per line — on a 500k-line file the Value
        // tree alone cost ~1GB of peak RSS. serde_json's default Map is a
        // BTreeMap, so keys serialize in sorted order; the byte layout below
        // (hash, lines, path / content, hash, n) matches `to_string` of the
        // equivalent `json!` object exactly.
        let out = ctx.stdout();
        write!(out, "{{\"hash\":")?;
        serde_json::to_writer(&mut *out, &fc.hash)?;
        write!(out, ",\"lines\":[")?;
        let mut first = true;
        for (i, entry) in entries.iter().enumerate() {
            if entry.content.is_empty() && i == raw_lines.len() - 1 && fc.trailing_newline {
                continue;
            }
            if !first {
                out.write_all(b",")?;
            }
            first = false;
            write!(out, "{{\"content\":")?;
            serde_json::to_writer(&mut *out, &entry.content)?;
            write!(out, ",\"hash\":")?;
            serde_json::to_writer(&mut *out, &format_short_hash(entry.short_hash))?;
            write!(out, ",\"n\":{}}}", i + 1)?;
        }
        write!(out, "],\"path\":")?;
        serde_json::to_writer(&mut *out, &fc.path.display().to_string())?;
        writeln!(out, "}}")?;
    } else {
        writeln!(
            ctx.stdout(),
            "[{}#{}]",
            crate::output::escape_compact_field(&fc.path.display().to_string()),
            fc.hash
        )?;
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
