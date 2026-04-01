use std::io::Write;

use crate::cli::AnnotateCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::annotate_lines;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: AnnotateCmd,
) -> Result<i32, LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let report = annotate_lines(&doc, &cmd.query, cmd.regex, cmd.expect_one)?;

    if report.exit_code != 0 {
        if cmd.json {
            output::write_grep_json(ctx, &report.lines)?;
        } else {
            output::write_success_line(
                ctx,
                &format!("annotate: expected 1 match, found {}", report.lines.len()),
            )?;
            output::print_line_views(ctx.stdout(), &report.lines)?;
        }
        return Ok(1);
    }

    if cmd.json {
        output::write_grep_json(ctx, &report.lines)?;
    } else if report.lines.is_empty() {
        output::write_success_line(ctx, "No matches found.")?;
    } else {
        output::print_line_views(ctx.stdout(), &report.lines)?;
    }

    Ok(0)
}
