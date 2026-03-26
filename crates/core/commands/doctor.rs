use std::io::Write;

use serde::Serialize;

use crate::cli::DoctorCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::output;

#[derive(Serialize)]
struct DoctorPayload<'a> {
    file: String,
    line_count: usize,
    estimated_read_tokens: usize,
    recommended_read_mode: &'a str,
    recommended_anchor_mode: &'a str,
    recommended_workflow: &'a str,
    suggested_context: usize,
    warnings: &'a [&'a str],
    next_commands: Vec<String>,
}

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: DoctorCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let stats = doc.compute_stats();
    let next_commands = build_next_commands(&cmd.file.display().to_string(), &stats);

    if cmd.json {
        let warnings = stats.warnings.to_vec();
        let payload = DoctorPayload {
            file: cmd.file.display().to_string(),
            line_count: stats.line_count,
            estimated_read_tokens: stats.estimated_read_tokens,
            recommended_read_mode: stats.recommended_read_mode,
            recommended_anchor_mode: stats.recommended_anchor_mode,
            recommended_workflow: stats.recommended_workflow,
            suggested_context: stats.suggested_context_n,
            warnings: &warnings,
            next_commands,
        };
        output::write_json_success(ctx, &payload)?;
    } else {
        output::write_success_line(ctx, &format!("File: {}", cmd.file.display()))?;
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
        for command in next_commands {
            output::write_success_line(ctx, &format!("- {command}"))?;
        }
    }

    Ok(())
}

fn build_next_commands(file: &str, stats: &crate::document::FileStats) -> Vec<String> {
    let mut commands = Vec::new();

    if stats.recommended_read_mode == "read" {
        commands.push(format!("linehash read {file}"));
    } else {
        commands.push(format!("linehash index {file}"));
        commands.push(format!(
            "linehash read {file} --anchor <line:hash> --context {}",
            stats.suggested_context_n
        ));
    }

    commands.push(format!("linehash annotate {file} <text>"));
    commands.push(format!("linehash grep {file} <pattern>"));

    if stats.collision_count > 0 || stats.line_count > 2_000 {
        commands.push(format!("linehash find-block {file} <line:hash>"));
        commands.push(format!("linehash patch {file} <patch.json> --dry-run"));
    } else {
        commands.push(format!("linehash verify {file} <line:hash>"));
        commands.push(format!("linehash edit {file} <line:hash> <new_content>"));
    }

    commands
}
