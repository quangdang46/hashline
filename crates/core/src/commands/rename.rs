use std::io::Write;

use crate::cli::RenameCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: RenameCmd,
) -> Result<(), HashlineError> {
    // 1. Check source exists
    if !cmd.src.exists() {
        return Err(HashlineError::FileNotFound {
            path: cmd.src.display().to_string(),
        });
    }

    // 2. Check destination does not already exist
    if cmd.dst.exists() {
        return Err(HashlineError::TargetExists {
            path: cmd.dst.display().to_string(),
        });
    }

    // 3. Create parent directories for destination if needed
    if let Some(parent) = cmd.dst.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 4. Perform the rename
    std::fs::rename(&cmd.src, &cmd.dst)?;

    // 5. Output result
    if cmd.json {
        let output = serde_json::json!({
            "success": true,
            "src": cmd.src.display().to_string(),
            "dst": cmd.dst.display().to_string(),
        });
        writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
    } else {
        writeln!(
            ctx.stdout(),
            "renamed '{}' -> '{}'",
            cmd.src.display(),
            cmd.dst.display()
        )?;
    }

    Ok(())
}
