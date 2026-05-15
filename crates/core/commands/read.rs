use std::io::Write;
use std::thread;

use crate::cli::ReadCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::LinehashError;
use crate::orchestration::{read_payload, resolve_read_anchors};
use crate::output;
use crate::search::persist::IndexStore;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ReadCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;

    if let Some(ref meta) = doc.file_meta {
        let path = cmd.file.canonicalize().unwrap_or_else(|_| cmd.file.clone());
        let mtime = meta.mtime_secs as u64;

        thread::spawn(move || {
            if let Ok(content) = std::fs::read(&path) {
                let root = path.parent().unwrap_or(&path);
                let store = IndexStore::new(root);
                let _ = store.write_index(&path, &content, mtime);
            }
        });
    }

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
