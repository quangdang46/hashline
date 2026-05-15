use std::io::Write;

use crate::cli::CallersCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::callgraph::search_callers_bfs;

/// Run callers command - find functions that call a given symbol.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: CallersCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let scope = cmd
        .scope
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new("."));

    let result = search_callers_bfs(&cmd.target, scope, cmd.depth);

    if cmd.json {
        let style = crate::output::JsonStyle::from_pretty(ctx.json_pretty());
        crate::output::serialize_json(ctx.stdout(), &result, style).map_err(LinehashError::Io)?;
    } else {
        writeln!(
            ctx.stdout(),
            "# Callers of '{}' (depth={})",
            cmd.target,
            cmd.depth
        )
        .map_err(LinehashError::Io)?;
        writeln!(ctx.stdout(), "# Found {} edges\n", result.edges.len())
            .map_err(LinehashError::Io)?;
        for edge in &result.edges {
            writeln!(
                ctx.stdout(),
                "{}:{} -> {} ({})",
                edge.from_file,
                edge.from_line,
                edge.to,
                edge.from
            )
            .map_err(LinehashError::Io)?;
        }
    }
    Ok(())
}
