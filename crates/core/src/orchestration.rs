use std::io::Write;

use tracing::debug;

use crate::cli::Commands;
use crate::commands;
use crate::context::{CommandContext, json_pretty_for, output_mode_for};
use crate::error::HashlineError;

pub fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Read(_) => "read",
        Commands::Patch(_) => "patch",
        Commands::FindBlock(_) => "find-block",
        Commands::Replace(_) => "replace",
        Commands::Guide(_) => "guide",
        Commands::Serve(_) => "serve",
        Commands::Mcp(_) => "mcp",
    }
}

use crate::cli::Cli;

pub fn run<W: Write, E: Write>(
    cli: Cli,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<i32, HashlineError> {
    run_command(cli.command, stdout, stderr).map(|(code, _)| code)
}

/// Execute `command` and return `(exit_code, _modified_doc)`.
pub fn run_command<W: Write, E: Write>(
    command: Commands,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(i32, Option<Vec<u8>>), HashlineError> {
    let output_mode = output_mode_for(&command);
    let json_pretty = json_pretty_for(&command);
    debug!(
        command = command_name(&command),
        ?output_mode,
        "dispatching command"
    );
    let mut context =
        CommandContext::new(stdout, stderr, output_mode).with_json_pretty(json_pretty);

    let exit_code = match command {
        Commands::Read(cmd) => commands::read::run(&mut context, cmd).map(|_| 0),
        Commands::Patch(cmd) => commands::patch::run(&mut context, cmd).map(|_| 0),
        Commands::Replace(cmd) => commands::replace::run(&mut context, cmd).map(|_| 0),
        Commands::FindBlock(cmd) => commands::find_block::run(&mut context, cmd).map(|_| 0),
        Commands::Guide(cmd) => commands::guide::run(&mut context, cmd).map(|_| 0),
        Commands::Serve(cmd) => commands::serve::run(&mut context, cmd).map(|_| 0),
        Commands::Mcp(_) => unreachable!("mcp mode is handled before command dispatch"),
    }?;

    Ok((exit_code, None))
}

#[cfg(test)]
mod tests {
    use super::command_name;
    use crate::cli::{Commands, PatchCmd, ReadCmd};
    use std::path::PathBuf;

    #[test]
    fn command_name_returns_read() {
        let cmd = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            json: false,
            no_cache: false,
        });
        assert_eq!(command_name(&cmd), "read");
    }

    #[test]
    fn command_name_returns_patch() {
        let cmd = Commands::Patch(PatchCmd {
            file: PathBuf::from("demo.txt"),
            patch: "".into(),
            dry_run: false,
            safe: false,
            json: false,
        });
        assert_eq!(command_name(&cmd), "patch");
    }
}
