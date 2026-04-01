use std::io::Write;

use crate::cli::ReadCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::{read_payload, resolve_read_anchors};
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReadCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;

    if cmd.json {
        let payload = read_payload(&doc, &cmd.anchor, cmd.context)?;
        output::print_read_json(ctx.stdout(), &payload)?;
        return Ok(());
    }

    if cmd.anchor.is_empty() {
        output::print_read(ctx.stdout(), &doc)?;
        return Ok(());
    }

    let resolved = resolve_read_anchors(&doc, &cmd.anchor)?;
    output::print_read_context(ctx.stdout(), &doc, &resolved, cmd.context)?;
    Ok(())
}
