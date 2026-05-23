use std::io::Write;

use crate::cli::ReadCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::hash_cache::discover_sidecar_root;
use crate::orchestration::{read_payload, resolve_read_anchors};
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReadCmd,
) -> Result<(), LinehashError> {
    // Pure read: use the hash sidecar so repeated reads of the same file
    // skip the per-line hashing pass on cache hit. First read writes the
    // sidecar in a background thread, so cold-cache latency is unchanged.
    let root = discover_sidecar_root(&cmd.file);
    let doc = Document::load_with_hash_cache(&cmd.file, &root)?;

    match ctx.output_mode() {
        OutputMode::Ndjson => {
            // Tier 1 NDJSON: one header + one object per line, no wrapper.
            // When no anchor filter is set we serialize straight from the
            // Document to skip the Vec<LineView> allocation that would
            // otherwise clone every line's content + hash into owned strings.
            if cmd.anchor.is_empty() && cmd.context == 0 {
                output::print_read_ndjson_streaming(ctx.stdout(), &doc)?;
            } else {
                let payload = read_payload(&doc, &cmd.anchor, cmd.context)?;
                output::print_read_ndjson(ctx.stdout(), &payload)?;
            }
            return Ok(());
        }
        OutputMode::Json => {
            let style = output::JsonStyle::from_pretty(ctx.json_pretty());
            if cmd.anchor.is_empty() && cmd.context == 0 {
                output::print_read_json_streaming(ctx.stdout(), &doc, style)?;
            } else {
                let payload = read_payload(&doc, &cmd.anchor, cmd.context)?;
                output::print_read_json(ctx.stdout(), &payload, style)?;
            }
            return Ok(());
        }
        OutputMode::Pretty => {}
    }

    if cmd.anchor.is_empty() {
        output::print_read(ctx.stdout(), &doc)?;
        return Ok(());
    }

    let resolved = resolve_read_anchors(&doc, &cmd.anchor)?;
    output::print_read_context(ctx.stdout(), &doc, &resolved, cmd.context)?;
    Ok(())
}
