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
        Commands::Read(cmd) => flag_mode(cmd.json),
        Commands::Patch(cmd) => flag_mode(cmd.json),
        Commands::Write(cmd) => flag_mode(cmd.json),
        Commands::FindBlock(cmd) => flag_mode(cmd.json),
        Commands::Guide(cmd) => flag_mode(cmd.json),
        Commands::Remove(cmd) => flag_mode(cmd.json),
        Commands::Rename(cmd) => flag_mode(cmd.json),
        Commands::Serve(_) | Commands::Mcp(_) => OutputMode::Pretty,
    }
}

/// Returns whether JSON output for `command` should be pretty-printed.
pub fn json_pretty_for(command: &Commands) -> bool {
    match command {
        Commands::Read(_) | Commands::Patch(_) | Commands::Write(_) => false,
        Commands::FindBlock(cmd) => cmd.pretty,
        Commands::Guide(_) | Commands::Remove(_) | Commands::Rename(_) => false,
        Commands::Serve(_) | Commands::Mcp(_) => false,
    }
}

fn flag_mode(json: bool) -> OutputMode {
    if json {
        OutputMode::Json
    } else {
        OutputMode::Pretty
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputMode, output_mode_for};
    use crate::cli::{Commands, FindBlockCmd, ReadCmd};
    use std::path::PathBuf;

    #[test]
    fn uses_json_mode_when_read_json_flag() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            json: true,
            no_cache: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn uses_pretty_mode_when_json_flag_is_false() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            json: false,
            no_cache: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn find_block_json_mode() {
        let command = Commands::FindBlock(FindBlockCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            json: true,
            pretty: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn patch_defaults_to_pretty() {
        let command = Commands::Patch(crate::cli::PatchCmd {
            file: PathBuf::from("demo.txt"),
            patch: "".into(),
            dry_run: false,
            safe: false,
            json: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn serve_defaults_to_pretty() {
        let command = Commands::Serve(crate::cli::ServeCmd {
            socket: None,
            http: None,
            detach: false,
            pid_file: None,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }
}
