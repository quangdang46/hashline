use std::io::Write;

use crate::cli::RemoveCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: RemoveCmd,
) -> Result<(), HashlineError> {
    // Shared pre-flight guards (also used by MCP `remove_file`):
    // device/reserved paths, existence, directory rejection.
    crate::commands::common::validate_remove_path(&cmd.file)?;

    // 2. Remove the file
    std::fs::remove_file(&cmd.file)?;

    // 3. Output result
    match ctx.output_mode() {
        crate::context::OutputMode::Compact | crate::context::OutputMode::Ndjson => {
            writeln!(
                ctx.stdout(),
                "OK {}",
                crate::output::escape_compact_field(&cmd.file.display().to_string())
            )?;
        }
        crate::context::OutputMode::Verbose => {
            writeln!(ctx.stdout(), "removed '{}'", cmd.file.display())?;
        }
        crate::context::OutputMode::Json => {
            let output = serde_json::json!({
                "success": true,
                "path": cmd.file.display().to_string(),
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
    }

    Ok(())
}
