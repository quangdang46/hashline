use std::io::Write;

use crate::cli::WorkflowsCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::output;
use crate::workflows::load_workflow_catalog;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: WorkflowsCmd,
) -> Result<(), LinehashError> {
    let root = match cmd.root {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    let catalog = load_workflow_catalog(&root)?;

    if cmd.json {
        return output::write_json_success(ctx, &catalog).map_err(LinehashError::from);
    }

    output::write_success_line(ctx, &format!("Root: {}", catalog.root))?;
    if catalog.packs.is_empty() {
        output::write_success_line(ctx, "Workflow packs: none")?;
        return Ok(());
    }

    output::write_success_line(ctx, "Workflow packs:")?;
    for pack in catalog.packs {
        output::write_success_line(ctx, &format!("- {} [{}]", pack.name, pack.source))?;
        output::write_success_line(ctx, &format!("  Title: {}", pack.title))?;
        output::write_success_line(ctx, &format!("  Description: {}", pack.description))?;
        output::write_success_line(ctx, &format!("  Surfaces: {}", pack.surfaces.join(", ")))?;
        output::write_success_line(
            ctx,
            &format!("  CLI: {}", pack.allowed_cli_commands.join(", ")),
        )?;
        output::write_success_line(
            ctx,
            &format!("  MCP: {}", pack.allowed_mcp_tools.join(", ")),
        )?;
    }

    Ok(())
}
