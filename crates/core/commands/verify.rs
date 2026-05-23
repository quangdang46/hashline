use std::io::Write;

use crate::cli::VerifyCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash_cache::discover_sidecar_root;
use crate::orchestration::verify_report;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: VerifyCmd,
) -> Result<i32, HashlineError> {
    let root = discover_sidecar_root(&cmd.file);
    let doc = Document::load_with_hash_cache(&cmd.file, &root)?;
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
