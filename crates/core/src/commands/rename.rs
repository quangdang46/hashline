use std::io::Write;

use crate::cli::RenameCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: RenameCmd,
) -> Result<(), HashlineError> {
    // 0-4. Shared pre-flight guards (also used by MCP `rename_file`):
    //     device/reserved paths, source must be an existing regular file,
    //     src/dst must not alias, destination handling per --force.
    crate::commands::common::validate_rename_paths(&cmd.src, &cmd.dst, cmd.force)?;

    // 4b. --force with an existing destination. On Unix, rename(2) already
    //     replaces the destination atomically — a pre-remove would only
    //     widen the crash window where BOTH files are gone
    //     (bd-mcp-serve-parity-batch-nh8w). Windows still needs the
    //     remove-first dance because MoveFile refuses existing targets.
    #[cfg(not(unix))]
    if cmd.force && cmd.dst.exists() {
        std::fs::remove_file(&cmd.dst)?;
    }

    // 5. Create parent directories for destination if needed
    if let Some(parent) = cmd.dst.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // 6. Perform the rename
    std::fs::rename(&cmd.src, &cmd.dst)?;

    // 7. Output result
    match ctx.output_mode() {
        crate::context::OutputMode::Compact | crate::context::OutputMode::Ndjson => {
            writeln!(
                ctx.stdout(),
                "OK {}>{}",
                crate::output::escape_compact_field(&cmd.src.display().to_string()),
                crate::output::escape_compact_field(&cmd.dst.display().to_string())
            )?;
        }
        crate::context::OutputMode::Verbose => {
            writeln!(
                ctx.stdout(),
                "renamed '{}' -> '{}'",
                cmd.src.display(),
                cmd.dst.display()
            )?;
        }
        crate::context::OutputMode::Json => {
            let output = serde_json::json!({
                "success": true,
                "src": cmd.src.display().to_string(),
                "dst": cmd.dst.display().to_string(),
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RenameCmd;
    use crate::context::{CommandContext, OutputMode};
    use std::fs;
    use std::path::PathBuf;

    fn rename_cmd(src: PathBuf, dst: PathBuf, force: bool) -> RenameCmd {
        RenameCmd {
            src,
            dst,
            json: false,
            verbose: false,
            force,
        }
    }

    fn run_rename(
        src: PathBuf,
        dst: PathBuf,
        force: bool,
    ) -> (Result<(), HashlineError>, Vec<u8>, Vec<u8>) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let result = {
            let mut ctx = CommandContext::new(&mut out, &mut err, OutputMode::Compact);
            run(&mut ctx, rename_cmd(src, dst, force))
        };
        (result, out, err)
    }

    #[test]
    fn rename_force_same_path_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();

        let (result, _out, _err) = run_rename(file.clone(), file.clone(), true);
        assert!(matches!(result, Err(HashlineError::SameFile { .. })));
        // The file must still be there with its content.
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn rename_force_dot_slash_alias_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();
        // `./f.txt` resolves to the same file.
        let dst = dir.path().join(".").join("f.txt");

        let (result, _out, _err) = run_rename(file.clone(), dst, true);
        assert!(matches!(result, Err(HashlineError::SameFile { .. })));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn rename_force_dotdot_alias_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let file = sub.join("f.txt");
        fs::write(&file, "hello").unwrap();
        // `sub/../sub/f.txt` resolves to the same file.
        let dst = sub.join("..").join("sub").join("f.txt");

        let (result, _out, _err) = run_rename(file.clone(), dst, true);
        assert!(matches!(result, Err(HashlineError::SameFile { .. })));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn rename_rejects_directory_source() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("srcdir");
        fs::create_dir(&src_dir).unwrap();
        let dst = dir.path().join("dstdir");

        let (result, _out, _err) = run_rename(src_dir.clone(), dst, true);
        assert!(matches!(result, Err(HashlineError::IsDirectory { .. })));
        assert!(src_dir.is_dir());
    }

    #[test]
    fn rename_rejects_directory_destination_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();
        let dst_dir = dir.path().join("dstdir");
        fs::create_dir(&dst_dir).unwrap();

        let (result, _out, _err) = run_rename(file.clone(), dst_dir.clone(), true);
        assert!(matches!(result, Err(HashlineError::IsDirectory { .. })));
        // Neither side was harmed.
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        assert!(dst_dir.is_dir());
    }

    #[test]
    fn rename_existing_destination_without_force_fails() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dst = dir.path().join("b.txt");
        fs::write(&src, "a").unwrap();
        fs::write(&dst, "b").unwrap();

        let (result, _out, _err) = run_rename(src.clone(), dst.clone(), false);
        assert!(matches!(result, Err(HashlineError::TargetExists { .. })));
        assert_eq!(fs::read_to_string(&src).unwrap(), "a");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "b");
    }

    #[test]
    fn rename_missing_source_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (result, _out, _err) = run_rename(
            dir.path().join("nope.txt"),
            dir.path().join("out.txt"),
            true,
        );
        assert!(matches!(result, Err(HashlineError::FileNotFound { .. })));
    }

    #[test]
    fn rename_normal_case_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dst = dir.path().join("b.txt");
        fs::write(&src, "content").unwrap();

        let (result, out, _err) = run_rename(src.clone(), dst.clone(), false);
        assert!(result.is_ok());
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "content");
        let stdout = String::from_utf8(out).unwrap();
        assert!(stdout.starts_with("OK "));
    }

    #[test]
    fn rename_force_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dst = dir.path().join("b.txt");
        fs::write(&src, "new").unwrap();
        fs::write(&dst, "old").unwrap();

        let (result, _out, _err) = run_rename(src, dst.clone(), true);
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
    }

    #[cfg(windows)]
    #[test]
    fn rename_force_case_variant_alias_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("File.TXT");
        fs::write(&file, "hello").unwrap();
        // Same name with different casing resolves to the same file.
        let dst = dir.path().join("file.txt");

        let (result, _out, _err) = run_rename(file.clone(), dst, true);
        assert!(matches!(result, Err(HashlineError::SameFile { .. })));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[cfg(windows)]
    #[test]
    fn rename_force_trailing_dot_alias_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "hello").unwrap();
        // Windows strips the trailing dot: `f.txt.` == `f.txt`.
        let dst = dir.path().join("f.txt.");

        let (result, _out, _err) = run_rename(file.clone(), dst, true);
        assert!(matches!(result, Err(HashlineError::SameFile { .. })));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }
}
