use std::io::Write;
use std::path::Path;

use crate::cli::OutlineCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::detect::{Lang, detect_language_from_path};
use crate::lang::outline::{OutlineEntry, get_outline_entries};
use serde::Serialize;

/// Run the outline command to get structural outline of a file.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: OutlineCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let content = std::fs::read_to_string(&cmd.file).map_err(|e| LinehashError::Io(e))?;

    let lang = detect_language_from_path(&cmd.file);
    let entries = get_outline_entries(&content, lang);

    if cmd.json {
        let payload = serde_json::to_string_pretty(&entries).map_err(|e| LinehashError::Json(e))?;
        writeln!(ctx.stdout(), "{}", payload).map_err(|e| LinehashError::Io(e))?;
    } else {
        for entry in &entries {
            writeln!(
                ctx.stdout(),
                "{}:{:?}:{}",
                entry.start_line,
                entry.kind,
                entry.name
            )
            .map_err(|e| LinehashError::Io(e))?;
        }
    }
    Ok(())
}
