use std::io::Write;

use crate::cli::GrepCmd;
use crate::context::CommandContext;
use crate::document::{Document, SearchDocument};
use crate::error::LinehashError;
use crate::orchestration::grep_lines;
use crate::output;
use crate::server;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: GrepCmd,
) -> Result<(), LinehashError> {
    if cmd.daemon {
        #[cfg(unix)]
        return run_via_daemon(ctx, cmd);
        #[cfg(not(unix))]
        {
            eprintln!("daemon mode is only supported on Unix");
            return Ok(());
        }
    }

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

#[cfg(unix)]
fn run_via_daemon<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: GrepCmd,
) -> Result<(), LinehashError> {
    if !server::is_daemon_running() {
        eprintln!("Starting daemon...");
        let _child = server::start_daemon()?;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if server::is_daemon_running() {
                break;
            }
        }
    }

    let request = server::Request::Grep {
        path: cmd.file.display().to_string(),
        pattern: cmd.pattern.clone(),
        invert: cmd.invert,
        case_insensitive: cmd.case_insensitive,
    };

    let data = server::client_request(&request)?;
    let lines: Vec<crate::orchestration::LineView> =
        serde_json::from_value(data).map_err(|e| LinehashError::ServerError {
            message: format!("failed to parse response: {}", e),
            kind: "parse_error".to_string(),
        })?;

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
