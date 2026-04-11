use std::io::Write;

use crate::cli::GrepCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::{grep_lines, grep_lines_indexed};
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: GrepCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let lines = if cmd.no_index {
        grep_lines(&doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
    } else {
        grep_lines_indexed(&doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
    };

    if cmd.json {
        output::write_grep_json(ctx, &lines)?;
    } else {
        output::print_line_views(ctx.stdout(), &lines)?;
    }

    Ok(())
}
