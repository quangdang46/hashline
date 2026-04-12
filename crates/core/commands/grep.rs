use std::io::Write;

use crate::cli::GrepCmd;
use crate::context::CommandContext;
use crate::document::{Document, SearchDocument};
use crate::error::LinehashError;
use crate::orchestration::grep_lines;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: GrepCmd,
) -> Result<(), LinehashError> {
    let use_fast_path = !cmd.case_insensitive && !contains_regex_metacharacters(&cmd.pattern);

    let lines = if use_fast_path {
        let search_doc = SearchDocument::load(&cmd.file)?;
        search_doc.grep_lines(&cmd.pattern, cmd.invert)
    } else {
        let doc = Document::load(&cmd.file)?;
        grep_lines(&doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
    };

    if cmd.json {
        output::write_grep_json(ctx, &lines)?;
    } else {
        output::print_line_views(ctx.stdout(), &lines)?;
    }

    Ok(())
}

fn contains_regex_metacharacters(s: &str) -> bool {
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
            | '"' => return true,
            _ => {}
        }
    }
    false
}
