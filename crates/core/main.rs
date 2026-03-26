mod anchor;
mod cli;
mod commands;
mod context;
mod document;
mod error;
mod hash;
mod mutation;
mod output;
mod receipt;

use std::io;
use std::io::Write;

use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::context::{CommandContext, output_mode_for};
use crate::error::LinehashError;

fn main() {
    let cli = Cli::parse();
    let output_mode = output_mode_for(&cli.command);
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    let exit_code = match run(cli, &mut stdout, &mut stderr) {
        Ok(code) => code,
        Err(error) => {
            let mut context = CommandContext::new(&mut stdout, &mut stderr, output_mode);
            let _ = output::write_error(&mut context, &error);
            1
        }
    };

    std::process::exit(exit_code);
}

fn run<W: Write, E: Write>(cli: Cli, stdout: &mut W, stderr: &mut E) -> Result<i32, LinehashError> {
    let output_mode = output_mode_for(&cli.command);
    let mut context = CommandContext::new(stdout, stderr, output_mode);

    match cli.command {
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
