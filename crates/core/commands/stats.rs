use std::io::Write;

use crate::cli::StatsCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: StatsCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let stats = doc.compute_stats();

    if cmd.json {
        output::write_json_success(ctx, &stats)?;
    } else {
        output::print_stats(ctx.stdout(), &stats)?;
    }

    Ok(())
}
