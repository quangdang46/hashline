use std::io::Write;

use crate::cli::OutlineCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::detect::detect_language_from_path;
use crate::lang::outline::get_outline_entries;

/// Run the outline command to get structural outline of a file.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: OutlineCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let content = std::fs::read_to_string(&cmd.file).map_err(LinehashError::Io)?;

    let lang = detect_language_from_path(&cmd.file);
    let entries = get_outline_entries(&content, lang);

    if cmd.json {
        let payload = serde_json::to_string_pretty(&entries).map_err(LinehashError::Json)?;
        writeln!(ctx.stdout(), "{}", payload).map_err(LinehashError::Io)?;
    } else {
        for entry in &entries {
            writeln!(
                ctx.stdout(),
                "{}:{:?}:{}",
                entry.start_line,
                entry.kind,
                entry.name
            )
            .map_err(LinehashError::Io)?;
        }
    }
    Ok(())
}
