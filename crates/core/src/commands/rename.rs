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

    // 2. Check destination does not already exist (unless --force)
    if cmd.dst.exists() {
        if cmd.force {
            std::fs::remove_file(&cmd.dst)?;
        } else {
            return Err(HashlineError::TargetExists {
                path: cmd.dst.display().to_string(),
            });
        }
    }

    // 3. Create parent directories for destination if needed
    if let Some(parent) = cmd.dst.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 4. Perform the rename
    std::fs::rename(&cmd.src, &cmd.dst)?;

    // 5. Output result
    match ctx.output_mode() {
        crate::context::OutputMode::Compact | crate::context::OutputMode::Ndjson => {
            writeln!(
                ctx.stdout(),
                "OK {}>{}",
                cmd.src.display(),
                cmd.dst.display()
            )?;
        }
        crate::context::OutputMode::Verbose => {
            writeln!(
                ctx.stdout(),
                "renamed '{}' -> '{}'",
                cmd.src.display(),
                cmd.dst.display()
            )?;
        }
        crate::context::OutputMode::Json => {
            let output = serde_json::json!({
                "success": true,
                "src": cmd.src.display().to_string(),
                "dst": cmd.dst.display().to_string(),
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
    }

    Ok(())
}
