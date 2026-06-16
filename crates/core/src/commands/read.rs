use std::io::Write;

use crate::cli::ReadCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReadCmd,
) -> Result<(), HashlineError> {
    let fc = FileContent::load(&cmd.file)?;

    if cmd.json {
        let raw_lines = fc.lines();
        let lines: Vec<serde_json::Value> = raw_lines
            .iter()
            .enumerate()
            .filter(|(i, line)| !(line.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline))
            .map(|(i, line)| {
                serde_json::json!({
                    "n": i + 1,
                    "content": line,
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
        writeln!(
            ctx.stdout(),
            "[{}#{}]",
            fc.path.display(),
            fc.hash
        )?;
        let lines = fc.lines();
        let count = lines.len();
        for (i, line) in lines.iter().enumerate() {
            // Skip the trailing empty line from split('\n') when file ends with '\n'
            if line.is_empty() && i == count - 1 && fc.trailing_newline {
                continue;
            }
            writeln!(ctx.stdout(), "{}|{}", i + 1, line)?;
        }
    }

    Ok(())
}
