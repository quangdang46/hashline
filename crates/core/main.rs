mod anchor;
mod cli;
mod commands;
mod context;
mod document;
mod error;
mod hash;
mod install;
mod mcp;
mod mutation;
mod output;
mod receipt;

use std::io;
use std::io::Write;

use clap::Parser;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands};
use crate::context::{CommandContext, output_mode_for};
use crate::error::LinehashError;

fn main() {
    let cli = Cli::parse();
    init_tracing();
    debug!(command = command_name(&cli.command), "parsed CLI arguments");

    if let Commands::Mcp(cmd) = &cli.command {
        info!("starting MCP server");
        if let Err(error) = mcp::run(cmd.clone()) {
            error!(%error, "mcp command failed");
            eprintln!("mcp error: {error}");
            std::process::exit(1);
        }
        info!("mcp server exited cleanly");
        return;
    }

    if let Commands::InstallMcp(_) = &cli.command {
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                error!(%error, "failed to determine current directory for install-mcp");
                eprintln!("install-mcp error: failed to determine current directory: {error}");
                std::process::exit(1);
            }
        };
        info!(cwd = %cwd.display(), "running install-mcp");
        if let Err(error) = install::run_install_mcp(&cwd, &mut io::stdout(), &mut io::stderr()) {
            error!(%error, cwd = %cwd.display(), "install-mcp failed");
            eprintln!("install-mcp error: {error}");
            std::process::exit(1);
        }
        info!("install-mcp completed");
        return;
    }

    let output_mode = output_mode_for(&cli.command);
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let exit_code = match run(cli, &mut stdout, &mut stderr) {
        Ok(code) => {
            info!(exit_code = code, "command completed");
            code
        }
        Err(error) => {
            error!(%error, "command failed");
            let mut context = CommandContext::new(&mut stdout, &mut stderr, output_mode);
            let _ = output::write_error(&mut context, &error);
            1
        }
    };

    std::process::exit(exit_code);
}

fn init_tracing() {
    let env_name = "LINEHASH_LOG";
    let configured = match std::env::var(env_name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(std::env::VarError::NotPresent) => return,
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("warning: {env_name} is not valid unicode; tracing is disabled");
            return;
        }
    };

    let filter = match EnvFilter::try_new(configured) {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!("warning: invalid {env_name} filter: {error}");
            return;
        }
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(false)
        .compact()
        .try_init();
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Read(_) => "read",
        Commands::Index(_) => "index",
        Commands::Edit(_) => "edit",
        Commands::Insert(_) => "insert",
        Commands::Delete(_) => "delete",
        Commands::Verify(_) => "verify",
        Commands::Grep(_) => "grep",
        Commands::Annotate(_) => "annotate",
        Commands::Patch(_) => "patch",
        Commands::Swap(_) => "swap",
        Commands::Move(_) => "move",
        Commands::Indent(_) => "indent",
        Commands::FindBlock(_) => "find-block",
        Commands::Stats(_) => "stats",
        Commands::Doctor(_) => "doctor",
        Commands::FromDiff(_) => "from-diff",
        Commands::MergePatches(_) => "merge-patches",
        Commands::Watch(_) => "watch",
        Commands::Explode(_) => "explode",
        Commands::Implode(_) => "implode",
        Commands::InstallMcp(_) => "install-mcp",
        Commands::Mcp(_) => "mcp",
    }
}

fn run<W: Write, E: Write>(cli: Cli, stdout: &mut W, stderr: &mut E) -> Result<i32, LinehashError> {
    run_command(cli.command, stdout, stderr)
}

pub(crate) fn run_command<W: Write, E: Write>(
    command: Commands,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<i32, LinehashError> {
    let output_mode = output_mode_for(&command);
    debug!(
        command = command_name(&command),
        ?output_mode,
        "dispatching command"
    );
    let mut context = CommandContext::new(stdout, stderr, output_mode);

    match command {
        Commands::Read(cmd) => commands::read::run(&mut context, cmd).map(|_| 0),
        Commands::Index(cmd) => commands::index::run(&mut context, cmd).map(|_| 0),
        Commands::Edit(cmd) => commands::edit::run(&mut context, cmd).map(|_| 0),
        Commands::Insert(cmd) => commands::insert::run(&mut context, cmd).map(|_| 0),
        Commands::Delete(cmd) => commands::delete::run(&mut context, cmd).map(|_| 0),
        Commands::Verify(cmd) => commands::verify::run(&mut context, cmd),
        Commands::Grep(cmd) => commands::grep::run(&mut context, cmd).map(|_| 0),
        Commands::Annotate(cmd) => commands::annotate::run(&mut context, cmd),
        Commands::Patch(cmd) => commands::patch::run(&mut context, cmd).map(|_| 0),
        Commands::Swap(cmd) => commands::swap::run(&mut context, cmd).map(|_| 0),
        Commands::Move(cmd) => commands::r#move::run(&mut context, cmd).map(|_| 0),
        Commands::Indent(cmd) => commands::indent::run(&mut context, cmd).map(|_| 0),
        Commands::FindBlock(cmd) => commands::find_block::run(&mut context, cmd).map(|_| 0),
        Commands::Stats(cmd) => commands::stats::run(&mut context, cmd).map(|_| 0),
        Commands::Doctor(cmd) => commands::doctor::run(&mut context, cmd).map(|_| 0),
        Commands::FromDiff(cmd) => commands::from_diff::run(&mut context, cmd).map(|_| 0),
        Commands::MergePatches(cmd) => commands::merge_patches::run(&mut context, cmd).map(|_| 0),
        Commands::Watch(cmd) => commands::watch::run(&mut context, cmd).map(|_| 0),
        Commands::Explode(cmd) => commands::explode::run(&mut context, cmd).map(|_| 0),
        Commands::Implode(cmd) => commands::implode::run(&mut context, cmd).map(|_| 0),
        Commands::InstallMcp(_) => unreachable!("install-mcp is handled before command dispatch"),
        Commands::Mcp(_) => unreachable!("mcp mode is handled before command dispatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::cli::{Cli, Commands, DoctorCmd, PatchCmd, ReadCmd};
    use std::path::PathBuf;

    #[test]
    fn pretty_errors_go_to_stderr_only() {
        let cli = Cli {
            command: Commands::Read(ReadCmd {
                file: PathBuf::from("missing.txt"),
                anchor: Vec::new(),
                context: 5,
                json: false,
            }),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run(cli, &mut stdout, &mut stderr);
        if let Err(error) = result {
            let mut sink_out = Vec::new();
            let mut sink_err = Vec::new();
            let mut ctx = crate::context::CommandContext::new(
                &mut sink_out,
                &mut sink_err,
                crate::context::OutputMode::Pretty,
            );
            crate::output::write_error(&mut ctx, &error).unwrap();
            stdout = sink_out;
            stderr = sink_err;
        }

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Error: I/O error:"));
        assert!(
            stderr.contains("Hint: check the file path and permissions, then retry the command")
        );
    }

    #[test]
    fn json_errors_are_machine_readable() {
        let cli = Cli {
            command: Commands::Patch(PatchCmd {
                file: PathBuf::from("foo"),
                patch: "bar".into(),
                dry_run: false,
                receipt: false,
                audit_log: None,
                expect_mtime: None,
                expect_inode: None,
                json: true,
            }),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let result = run(cli, &mut stdout, &mut stderr);
        if let Err(error) = result {
            let mut sink_out = Vec::new();
            let mut sink_err = Vec::new();
            let mut ctx = crate::context::CommandContext::new(
                &mut sink_out,
                &mut sink_err,
                crate::context::OutputMode::Json,
            );
            crate::output::write_error(&mut ctx, &error).unwrap();
            stdout = sink_out;
            stderr = sink_err;
        }

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stderr).unwrap();
        assert!(parsed["error"].as_str().unwrap().starts_with("I/O error:"));
        assert_eq!(
            parsed["hint"],
            "check the file path and permissions, then retry the command"
        );
    }

    #[test]
    fn doctor_uses_json_mode_when_requested() {
        let cli = Cli {
            command: Commands::Doctor(DoctorCmd {
                file: PathBuf::from("demo.txt"),
                json: true,
            }),
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(cli, &mut stdout, &mut stderr);
        assert!(result.is_err());
    }
}
