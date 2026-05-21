use std::io::Write;

use crate::cli::DoctorCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::hash_cache::discover_sidecar_root;
use crate::orchestration::doctor_payload;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: DoctorCmd,
) -> Result<(), LinehashError> {
    let root = discover_sidecar_root(&cmd.file);
    let doc = Document::load_with_hash_cache(&cmd.file, &root)?;
    let stats = doc.compute_stats();
    let payload = doctor_payload(&cmd.file, &stats);

    if cmd.json {
        output::write_json_success(ctx, &payload)?;
    } else {
        output::write_success_line(ctx, &format!("File: {}", payload.file))?;
        output::write_success_line(ctx, &format!("Lines: {}", stats.line_count))?;
        output::write_success_line(
            ctx,
            &format!(
                "Estimated full read cost: ~{} tokens",
                stats.estimated_read_tokens
            ),
        )?;
        output::write_success_line(
            ctx,
            &format!("Recommended read mode: {}", stats.recommended_read_mode),
        )?;
        output::write_success_line(
            ctx,
            &format!("Recommended anchor mode: {}", stats.recommended_anchor_mode),
        )?;
        output::write_success_line(
            ctx,
            &format!("Suggested --context: {}", stats.suggested_context_n),
        )?;
        output::write_success_line(
            ctx,
            &format!("Recommended workflow: {}", stats.recommended_workflow),
        )?;
        if stats.warnings.is_empty() {
            output::write_success_line(ctx, "Warnings: none")?;
        } else {
            output::write_success_line(ctx, "Warnings:")?;
            for warning in &stats.warnings {
                output::write_success_line(ctx, &format!("- {warning}"))?;
            }
        }
        output::write_success_line(ctx, "Next commands:")?;
        for command in payload.next_commands {
            output::write_success_line(ctx, &format!("- {command}"))?;
        }
    }

    Ok(())
}
