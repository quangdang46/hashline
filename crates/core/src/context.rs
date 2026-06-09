use std::io::Write;

use crate::cli::Commands;

/// Coarse output mode. JSON style (compact vs pretty) is tracked separately on
/// [`CommandContext`] via [`CommandContext::json_pretty`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    /// Human-readable text output (default).
    Pretty,
    /// Single JSON document (compact by default; pretty when `--pretty` is set).
    Json,
    /// Newline-delimited JSON stream (one JSON object per line, no wrapper).
    Ndjson,
}

pub struct CommandContext<'a, W: Write, E: Write> {
    stdout: &'a mut W,
    stderr: &'a mut E,
    output_mode: OutputMode,
    json_pretty: bool,
}

impl<'a, W: Write, E: Write> CommandContext<'a, W, E> {
    pub fn new(stdout: &'a mut W, stderr: &'a mut E, output_mode: OutputMode) -> Self {
        Self {
            stdout,
            stderr,
            output_mode,
            json_pretty: false,
        }
    }

    /// Builder helper: enable pretty-printing for JSON output.
    /// Has no effect on `OutputMode::Pretty` (text) or `OutputMode::Ndjson`.
    pub fn with_json_pretty(mut self, pretty: bool) -> Self {
        self.json_pretty = pretty;
        self
    }

    pub fn stdout(&mut self) -> &mut W {
        self.stdout
    }

    pub fn stderr(&mut self) -> &mut E {
        self.stderr
    }

    pub fn output_mode(&self) -> OutputMode {
        self.output_mode
    }

    /// Whether JSON output should be pretty-printed. Compact by default.
    pub fn json_pretty(&self) -> bool {
        self.json_pretty
    }
}

pub fn output_mode_for(command: &Commands) -> OutputMode {
    match command {
        Commands::Read(cmd) => format_mode(cmd.json, cmd.ndjson),
        Commands::Index(cmd) => format_mode(cmd.json, cmd.ndjson),
        Commands::Grep(cmd) => format_mode(cmd.json, cmd.ndjson),
        Commands::Edit(cmd) => flag_mode(cmd.json),
        Commands::Verify(cmd) => flag_mode(cmd.json),
        Commands::Insert(cmd) => flag_mode(cmd.json),
        Commands::Delete(cmd) => flag_mode(cmd.json),
        Commands::Patch(cmd) => flag_mode(cmd.json),
        Commands::Indent(cmd) => flag_mode(cmd.json),
        Commands::Stats(cmd) => flag_mode(cmd.json),
        Commands::Doctor(cmd) => flag_mode(cmd.json),
        Commands::Swap(_) | Commands::Move(_) | Commands::Mcp(_) => OutputMode::Pretty,
    }
}

/// Returns whether JSON output for `command` should be pretty-printed.
/// `--ndjson` and text-mode commands always return `false`.
pub fn json_pretty_for(command: &Commands) -> bool {
    match command {
        Commands::Read(cmd) => json_pretty_flag(cmd.json, cmd.pretty, cmd.ndjson),
        Commands::Index(cmd) => json_pretty_flag(cmd.json, cmd.pretty, cmd.ndjson),
        Commands::Grep(cmd) => json_pretty_flag(cmd.json, cmd.pretty, cmd.ndjson),
        Commands::Edit(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Verify(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Insert(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Delete(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Patch(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Indent(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Stats(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Doctor(cmd) => json_pretty_flag(cmd.json, cmd.pretty, false),
        Commands::Swap(_) | Commands::Move(_) | Commands::Mcp(_) => false,
    }
}

fn flag_mode(json: bool) -> OutputMode {
    if json {
        OutputMode::Json
    } else {
        OutputMode::Pretty
    }
}

/// `--ndjson` wins over `--json`. Otherwise `--json` selects single-document JSON.
fn format_mode(json: bool, ndjson: bool) -> OutputMode {
    if ndjson {
        OutputMode::Ndjson
    } else if json {
        OutputMode::Json
    } else {
        OutputMode::Pretty
    }
}

/// Compute json_pretty: only meaningful when JSON mode is selected (not ndjson, not text).
fn json_pretty_flag(json: bool, pretty: bool, ndjson: bool) -> bool {
    json && pretty && !ndjson
}

#[cfg(test)]
mod tests {
    use super::{OutputMode, json_pretty_for, output_mode_for};
    use crate::cli::{Commands, DeleteCmd, DoctorCmd, EditCmd, IndentCmd, InsertCmd, ReadCmd};
    use std::path::PathBuf;

    #[test]
    fn uses_json_mode_when_command_requests_it() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: true,
            pretty: false,
            ndjson: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
        assert!(!json_pretty_for(&command));
    }

    #[test]
    fn uses_pretty_mode_when_json_flag_is_false() {
        let command = Commands::Edit(EditCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            content: "new".into(),
            dry_run: false,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            interpret_escapes: false,
            json: false,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn supports_json_mode_for_insert() {
        let command = Commands::Insert(InsertCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            content: "new".into(),
            before: false,
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            interpret_escapes: false,
            json: true,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_indent() {
        let command = Commands::Indent(IndentCmd {
            file: PathBuf::from("demo.txt"),
            range: "1:aa..2:bb".into(),
            amount: "+2".into(),
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: true,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_delete() {
        let command = Commands::Delete(DeleteCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: true,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_doctor() {
        let command = Commands::Doctor(DoctorCmd {
            file: PathBuf::from("demo.txt"),
            json: true,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn pretty_flag_enables_pretty_json() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: true,
            pretty: true,
            ndjson: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
        assert!(json_pretty_for(&command));
    }

    #[test]
    fn pretty_flag_without_json_has_no_effect() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: false,
            pretty: true,
            ndjson: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
        assert!(!json_pretty_for(&command));
    }

    #[test]
    fn ndjson_flag_overrides_json() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: true,
            pretty: true,
            ndjson: true,
        });

        // ndjson wins over json/pretty
        assert_eq!(output_mode_for(&command), OutputMode::Ndjson);
        assert!(!json_pretty_for(&command));
    }
}
