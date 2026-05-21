use std::io::Write;

use crate::cli::IndexCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::hash_cache::discover_sidecar_root;
use crate::orchestration::index_payload;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: IndexCmd,
) -> Result<(), LinehashError> {
    let root = discover_sidecar_root(&cmd.file);
    let doc = Document::load_with_hash_cache(&cmd.file, &root)?;

    match ctx.output_mode() {
        OutputMode::Ndjson => {
            let payload = index_payload(&doc);
            output::print_index_ndjson(ctx.stdout(), &payload)?;
        }
        OutputMode::Json => {
            let payload = index_payload(&doc);
            let style = output::JsonStyle::from_pretty(ctx.json_pretty());
            output::print_index_json(ctx.stdout(), &payload, style)?;
        }
        OutputMode::Pretty => {
            output::print_index(ctx.stdout(), &doc)?;
        }
    }

    Ok(())
}
