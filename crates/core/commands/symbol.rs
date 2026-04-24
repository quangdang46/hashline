use std::io::Write;
use std::path::Path;

use crate::cli::SymbolCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::detect::detect_language_from_path;
use crate::lang::symbol::{SymbolResult, extract_symbols};

/// Run symbol search command.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: SymbolCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let scope = cmd.scope.as_deref().unwrap_or_else(|| Path::new("."));
    let query = &cmd.query;

    // Find matching symbols
    let mut result = SymbolResult::new(query);

    // Walk files in scope
    let walker = ignore::WalkBuilder::new(scope)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let lang = detect_language_from_path(path);
        if !lang.is_source() {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let symbols = extract_symbols(&content, lang, path);

        for sym in symbols {
            if sym.name.to_lowercase().contains(&query.to_lowercase()) {
                result.matches.push(sym);
            }
        }
    }

    result.total = result.matches.len();

    if cmd.json {
        serde_json::to_writer_pretty(ctx.stdout(), &result).map_err(LinehashError::Json)?;
        writeln!(ctx.stdout()).map_err(LinehashError::Io)?;
    } else {
        for m in &result.matches {
            writeln!(
                ctx.stdout(),
                "{}:{}:{:?}:{}",
                m.file,
                m.line,
                m.kind,
                m.name
            )
            .map_err(LinehashError::Io)?;
        }
    }
    Ok(())
}
