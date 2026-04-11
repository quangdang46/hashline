use std::io::Write;
use std::thread;

use crate::cli::ReadCmd;
use crate::context::CommandContext;
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
