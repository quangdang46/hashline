#![allow(dead_code)]

use std::io::{self, Write};

use serde::Serialize;

use crate::context::{CommandContext, OutputMode};
use crate::error::HashlineError;

/// Whether to emit JSON in compact (default) or pretty form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonStyle {
    Compact,
    Pretty,
}

impl JsonStyle {
    pub fn from_pretty(pretty: bool) -> Self {
        if pretty {
            JsonStyle::Pretty
        } else {
            JsonStyle::Compact
        }
    }
}

/// Serialize `value` to `writer` followed by a single newline, using the
/// requested JSON style (compact by default, pretty when explicitly opted in).
pub fn serialize_json<W: Write, T: Serialize + ?Sized>(
    writer: &mut W,
    value: &T,
    style: JsonStyle,
) -> io::Result<()> {
    match style {
        JsonStyle::Compact => serde_json::to_writer(&mut *writer, value)?,
        JsonStyle::Pretty => serde_json::to_writer_pretty(&mut *writer, value)?,
    }
    writeln!(writer)
}

/// Emit `value` as JSON to stdout, using the context's JSON style.
#[allow(dead_code)]
pub fn write_json_success<W: Write, E: Write, T: Serialize + ?Sized>(
    ctx: &mut CommandContext<'_, W, E>,
    value: &T,
) -> io::Result<()> {
    let style = JsonStyle::from_pretty(ctx.json_pretty());
    serialize_json(ctx.stdout(), value, style)
}

#[derive(Serialize)]
struct ErrorPayload<'a> {
    /// Machine-readable error kind (`STALE_ANCHOR`, `NOOP_LOOP`, ...).
    kind: &'a str,
    error: String,
    hint: Option<&'a str>,
    command: Option<&'a str>,
}

pub fn write_error<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    error: &HashlineError,
) -> io::Result<()> {
    match ctx.output_mode() {
        OutputMode::Pretty => {
            writeln!(ctx.stderr(), "Error: {error}")?;
            if let Some(hint) = error.hint() {
                writeln!(ctx.stderr(), "Hint: {hint}")?;
            }
            Ok(())
        }
        OutputMode::Json | OutputMode::Ndjson => {
            let payload = ErrorPayload {
                kind: error.kind(),
                error: error.to_string(),
                hint: error.hint(),
                command: error.command(),
            };
            let style = if matches!(ctx.output_mode(), OutputMode::Json) && ctx.json_pretty() {
                JsonStyle::Pretty
            } else {
                JsonStyle::Compact
            };
            serialize_json(ctx.stderr(), &payload, style)
        }
    }
}
