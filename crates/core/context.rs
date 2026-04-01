use std::io::Write;

use crate::cli::Commands;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    Pretty,
    Json,
}

pub struct CommandContext<'a, W: Write, E: Write> {
    stdout: &'a mut W,
    stderr: &'a mut E,
    output_mode: OutputMode,
}

impl<'a, W: Write, E: Write> CommandContext<'a, W, E> {
    pub fn new(stdout: &'a mut W, stderr: &'a mut E, output_mode: OutputMode) -> Self {
        Self {
            stdout,
            stderr,
            output_mode,
        }
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
}

pub fn output_mode_for(command: &Commands) -> OutputMode {
    match command {
        Commands::Read(cmd) => flag_mode(cmd.json),
        Commands::Index(cmd) => flag_mode(cmd.json),
        Commands::Edit(cmd) => flag_mode(cmd.json),
        Commands::Verify(cmd) => flag_mode(cmd.json),
        Commands::Grep(cmd) => flag_mode(cmd.json),
        Commands::Annotate(cmd) => flag_mode(cmd.json),
        Commands::Insert(cmd) => flag_mode(cmd.json),
        Commands::Delete(cmd) => flag_mode(cmd.json),
        Commands::Patch(cmd) => flag_mode(cmd.json),
        Commands::Indent(cmd) => flag_mode(cmd.json),
        Commands::FindBlock(cmd) => flag_mode(cmd.json),
        Commands::Stats(cmd) => flag_mode(cmd.json),
        Commands::Doctor(cmd) => flag_mode(cmd.json),
        Commands::Workflows(cmd) => flag_mode(cmd.json),
        Commands::FromDiff(cmd) => flag_mode(cmd.json),
        Commands::MergePatches(cmd) => flag_mode(cmd.json),
        Commands::Watch(cmd) => flag_mode(cmd.json),
        Commands::WatchCapabilities(cmd) => flag_mode(cmd.json),
        Commands::Swap(_)
        | Commands::Move(_)
        | Commands::Explode(_)
        | Commands::Implode(_)
        | Commands::InstallMcp(_)
        | Commands::Mcp(_) => OutputMode::Pretty,
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
    use crate::cli::{
        Commands, DeleteCmd, DoctorCmd, EditCmd, ExplodeCmd, ImplodeCmd, IndentCmd, InsertCmd,
        ReadCmd, WatchCapabilitiesCmd, WatchCmd, WorkflowsCmd,
    };
    use std::path::PathBuf;

    #[test]
    fn uses_json_mode_when_command_requests_it() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
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
            json: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn defaults_to_pretty_for_commands_without_json_flag() {
        let command = Commands::Explode(ExplodeCmd {
            file: PathBuf::from("demo.txt"),
            out: PathBuf::from("out"),
            force: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn implode_defaults_to_pretty_mode() {
        let command = Commands::Implode(ImplodeCmd {
            dir: PathBuf::from("exploded"),
            out: PathBuf::from("demo.txt"),
            dry_run: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn supports_json_mode_for_watch() {
        let command = Commands::Watch(WatchCmd {
            file: PathBuf::from("demo.txt"),
            once: false,
            continuous: true,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_watch_capabilities() {
        let command = Commands::WatchCapabilities(WatchCapabilitiesCmd { json: true });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_workflows() {
        let command = Commands::Workflows(WorkflowsCmd {
            root: Some(PathBuf::from(".")),
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
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
            json: true,
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
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_doctor() {
        let command = Commands::Doctor(DoctorCmd {
            file: PathBuf::from("demo.txt"),
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }
}
