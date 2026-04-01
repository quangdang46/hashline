use std::io::Write;

use crate::cli::VerifyCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::verify_report;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: VerifyCmd,
) -> Result<i32, LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let report = verify_report(&doc, &cmd.anchors);

    if cmd.json {
        output::write_json_success(ctx, &report.results)?;
    } else {
        for result in &report.results {
            match result.status {
                "ok" => output::write_success_line(
                    ctx,
                    &format!(
                        "✓  {}  resolves → {:?}",
                        result.anchor,
                        result.content.as_deref().unwrap_or("")
                    ),
                )?,
                _ => output::write_success_line(
                    ctx,
                    &format!(
                        "✗  {}  {}",
                        result.anchor,
                        result.error.as_deref().unwrap_or("unknown error")
                    ),
                )?,
            }
        }
    }

    Ok(report.exit_code)
}
