mod anchor;
mod cli;
mod commands;
mod context;
mod document;
mod error;
mod hash;
mod index;
mod install;
mod lang;
mod mcp;
mod mutation;
mod orchestration;
mod output;
mod receipt;
mod risk;
mod search;
#[cfg(unix)]
mod server;
mod workflows;

use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use clap::Parser;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriter;

use crate::cli::{Cli, Commands};
use crate::context::{CommandContext, SearchDocCache, json_pretty_for, output_mode_for};
use crate::error::LinehashError;
use crate::orchestration::command_name;
use crate::risk::assess_command;

fn main() {
    let cli = Cli::parse();
    init_tracing();
    debug!(command = command_name(&cli.command), "parsed CLI arguments");
    info!(command = command_name(&cli.command), "command started");

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
    let json_pretty = json_pretty_for(&cli.command);
    // Wrap stdout/stderr in BufWriter so large outputs (e.g. `read --json`
    // on a 200k-line file) don't take one syscall per write — serde_json
    // emits many small writes per record, and a fresh `io::stdout()` handle
    // is not block-buffered when piped.
    let stdout_lock = io::stdout().lock();
    let mut stdout = io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
    let stderr_lock = io::stderr().lock();
    let mut stderr = io::BufWriter::with_capacity(8 * 1024, stderr_lock);

    let exit_code = match run(cli, &mut stdout, &mut stderr) {
        Ok(code) => {
            info!(exit_code = code, "command completed");
            code
        }
        Err(error) => {
            if error.log_as_error() {
                error!(%error, "command failed");
            } else {
                warn!(%error, "command rejected");
            }
            let mut context = CommandContext::new(
                &mut stdout,
                &mut stderr,
                output_mode,
                SearchDocCache::new(64),
            )
            .with_json_pretty(json_pretty);
            let _ = output::write_error(&mut context, &error);
            1
        }
    };

    // `process::exit` skips destructors, so the BufWriters above must be
    // explicitly flushed or buffered output is silently dropped. We ignore
    // flush errors — there is no useful action to take if writing to a
    // closed pipe fails at process exit.
    let _ = stdout.flush();
    let _ = stderr.flush();

    std::process::exit(exit_code);
}

fn init_tracing() {
    let filter = match tracing_filter() {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!("warning: invalid LINEHASH_LOG filter: {error}");
            return;
        }
    };

    let log_path = match resolve_log_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("warning: tracing log path unavailable: {error}");
            return;
        }
    };

    let writer = match SharedFileWriter::new(&log_path) {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!(
                "warning: failed to open linehash log file {}: {error}",
                log_path.display()
            );
            return;
        }
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .compact()
        .try_init();
}

fn tracing_filter() -> Result<EnvFilter, tracing_subscriber::filter::ParseError> {
    match std::env::var("LINEHASH_LOG") {
        Ok(value) if !value.trim().is_empty() => EnvFilter::try_new(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => EnvFilter::try_new("info"),
        Err(std::env::VarError::NotUnicode(_)) => EnvFilter::try_new("info"),
    }
}

fn resolve_log_path() -> Result<PathBuf, String> {
    match std::env::var("LINEHASH_LOG_PATH") {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(default_log_path(linehash_home_dir()?)),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("LINEHASH_LOG_PATH is not valid unicode".into())
        }
    }
}

fn linehash_home_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .map_err(|_| "USERPROFILE not set".into())
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| "HOME not set".into())
    }
}

fn default_log_path(home: PathBuf) -> PathBuf {
    home.join(".linehash").join("linehash.log")
}

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<std::fs::File>>,
}

impl SharedFileWriter {
    fn new(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }
}

struct SharedFileGuard<'a> {
    guard: MutexGuard<'a, std::fs::File>,
}

impl Write for SharedFileGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.guard.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.guard.flush()
    }
}

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = SharedFileGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SharedFileGuard {
            guard: self
                .file
                .lock()
                .expect("linehash tracing file lock poisoned"),
        }
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
    let json_pretty = json_pretty_for(&command);
    let risk = assess_command(&command);
    debug!(
        command = command_name(&command),
        ?output_mode,
        "dispatching command"
    );
    if let Some(risk) = risk.as_ref() {
        info!(
            command = command_name(&command),
            risk_level = risk.level.as_str(),
            risk_summary = %risk.summary,
            "destructive command risk assessed"
        );
    }
    let mut context = CommandContext::new(stdout, stderr, output_mode, SearchDocCache::new(64))
        .with_json_pretty(json_pretty);

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
        Commands::Workflows(cmd) => commands::workflows::run(&mut context, cmd).map(|_| 0),
        Commands::FromDiff(cmd) => commands::from_diff::run(&mut context, cmd).map(|_| 0),
        Commands::MergePatches(cmd) => commands::merge_patches::run(&mut context, cmd).map(|_| 0),
        Commands::Watch(cmd) => commands::watch::run(&mut context, cmd).map(|_| 0),
        Commands::WatchCapabilities(cmd) => {
            commands::watch_capabilities::run(&mut context, cmd).map(|_| 0)
        }
        Commands::Explode(cmd) => commands::explode::run(&mut context, cmd).map(|_| 0),
        Commands::Implode(cmd) => commands::implode::run(&mut context, cmd).map(|_| 0),
        Commands::Map(cmd) => commands::map::run(&mut context, cmd).map(|_| 0),
        Commands::Outline(cmd) => commands::outline::run(&mut context, cmd).map(|_| 0),
        Commands::Symbol(cmd) => commands::symbol::run(&mut context, cmd).map(|_| 0),
        Commands::Callers(cmd) => commands::callers::run(&mut context, cmd).map(|_| 0),
        Commands::Callees(cmd) => commands::callees::run(&mut context, cmd).map(|_| 0),
        Commands::Deps(cmd) => commands::deps::run(&mut context, cmd).map(|_| 0),
        Commands::InstallMcp(_) => unreachable!("install-mcp is handled before command dispatch"),
        Commands::Mcp(_) => unreachable!("mcp mode is handled before command dispatch"),
        #[cfg(unix)]
        Commands::Daemon => {
            info!("starting daemon mode");
            if let Err(error) = server::run_daemon() {
                error!(%error, "daemon failed");
                eprintln!("daemon error: {error}");
                return Err(error);
            }
            Ok(0)
        }
        #[cfg(not(unix))]
        Commands::Daemon => {
            eprintln!("daemon mode is only supported on Unix");
            Err(LinehashError::Io(std::io::Error::other(
                "daemon not supported on this platform",
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{default_log_path, run, tracing_filter};
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
                pretty: false,
                ndjson: false,
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
                crate::context::SearchDocCache::new(0),
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
                pretty: false,
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
                crate::context::SearchDocCache::new(0),
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
                pretty: false,
            }),
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(cli, &mut stdout, &mut stderr);
        assert!(result.is_err());
    }

    #[test]
    fn default_log_path_is_under_linehash_home_dir() {
        let path = default_log_path(PathBuf::from("/tmp/test-home"));
        assert_eq!(path, PathBuf::from("/tmp/test-home/.linehash/linehash.log"));
    }

    #[test]
    fn tracing_filter_defaults_to_info() {
        let filter = tracing_filter().unwrap();
        let rendered = filter.to_string();
        assert!(rendered.contains("info"));
    }
}
