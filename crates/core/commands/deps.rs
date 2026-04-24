use std::io::Write;

use crate::cli::DepsCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::deps::{DepsResult, extract_imports};
use crate::lang::detect::detect_language_from_path;

/// Run deps command - show dependency analysis.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: DepsCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let scope = cmd
        .scope
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new("."));

    if let Some(file) = cmd.file.as_deref() {
        // Show deps for specific file
        let content = std::fs::read_to_string(file).map_err(LinehashError::Io)?;
        let lang = detect_language_from_path(file);
        let imports = extract_imports(&content, lang, file);

        if cmd.json {
            let result = DepsResult {
                file: file.to_string_lossy().to_string(),
                imports,
                imported_by: Vec::new(),
            };
            serde_json::to_writer_pretty(ctx.stdout(), &result).map_err(LinehashError::Json)?;
            writeln!(ctx.stdout()).map_err(LinehashError::Io)?;
        } else {
            writeln!(ctx.stdout(), "# Dependencies for {}", file.display())
                .map_err(LinehashError::Io)?;
            writeln!(ctx.stdout(), "\n## Imports:\n").map_err(LinehashError::Io)?;
            for imp in &imports {
                let alias_str = imp
                    .alias
                    .as_ref()
                    .map(|a| format!(" as {}", a))
                    .unwrap_or_default();
                writeln!(
                    ctx.stdout(),
                    "  {}:{} {}{}",
                    imp.line,
                    imp.kind_debug(),
                    imp.path,
                    alias_str
                )
                .map_err(LinehashError::Io)?;
            }
        }
    } else {
        // Show all imports in scope
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

            let imports = extract_imports(&content, lang, path);
            for imp in imports {
                writeln!(ctx.stdout(), "{}:{}:{}", path.display(), imp.line, imp.path)
                    .map_err(LinehashError::Io)?;
            }
        }
    }
    Ok(())
}
