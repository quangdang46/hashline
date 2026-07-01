use std::io::Write;

use crate::cli::RemoveCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: RemoveCmd,
) -> Result<(), HashlineError> {
    // 1. Check file exists
    if !cmd.file.exists() {
        return Err(HashlineError::FileNotFound {
            path: cmd.file.display().to_string(),
        });
    }

    // 2. Remove the file
    std::fs::remove_file(&cmd.file)?;

    // 3. Output result
    if cmd.json {
        let output = serde_json::json!({
            "success": true,
            "path": cmd.file.display().to_string(),
        });
        writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
    } else {
        writeln!(ctx.stdout(), "removed '{}'", cmd.file.display())?;
    }

    Ok(())
}
