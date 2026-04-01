use std::io::Write;

use crate::cli::WatchCapabilitiesCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::orchestration::watch_capabilities_payload;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: WatchCapabilitiesCmd,
) -> Result<(), LinehashError> {
    let payload = watch_capabilities_payload();

    if cmd.json {
        return output::write_json_success(ctx, &payload).map_err(LinehashError::from);
    }

    output::write_success_line(
        ctx,
        &format!(
            "CLI continuous watch: {}",
            if payload.cli_continuous_supported {
                "supported"
            } else {
                "not supported"
            }
        ),
    )?;
    output::write_success_line(
        ctx,
        &format!(
            "MCP single-event watch: {}",
            if payload.mcp_single_event_supported {
                "supported"
            } else {
                "not supported"
            }
        ),
    )?;
    output::write_success_line(
        ctx,
        &format!(
            "MCP streaming watch: {}",
            if payload.mcp_streaming_supported {
                "supported"
            } else {
                "not supported"
            }
        ),
    )?;
    output::write_success_line(
        ctx,
        &format!("Recommended MCP mode: {}", payload.recommended_mcp_mode),
    )?;
    output::write_success_line(ctx, &format!("Reason: {}", payload.streaming_block_reason))?;
    output::write_success_line(ctx, "Alternatives:")?;
    for alternative in payload.recommended_alternatives {
        output::write_success_line(ctx, &format!("- {alternative}"))?;
    }

    Ok(())
}
