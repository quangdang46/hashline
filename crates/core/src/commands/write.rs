use std::io::Write;

use crate::cli::WriteCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;
use crate::hash;
use crate::normalize;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: WriteCmd,
) -> Result<(), HashlineError> {
    // 0. Reject device/reserved paths BEFORE any filesystem probe —
    //    `exists()`/`metadata()` on a DOS device name (CON, AUX, ...) can
    //    block forever opening the device.
    crate::commands::common::reject_special_write_path(&cmd.file)?;

    // 1. Check file exists — refuse unless --force
    if cmd.file.exists() && !cmd.force {
        return Err(HashlineError::TargetExists {
            path: cmd.file.display().to_string(),
        });
    }

    // 2. Normalize content (LF line endings, strip BOM)
    let normalized = normalize::normalize_to_lf(&cmd.content);
    let bom_result = normalize::strip_bom(&normalized);
    let write_content = bom_result.text;

    // 2b. `write --force` on an existing file preserves that file's BOM
    //     marker and dominant line-ending style, so overwriting does not
    //     silently change the file's encoding conventions. New files (and
    //     files that are missing/unreadable/non-UTF-8) are written LF/no-BOM.
    let (bom, line_ending) = crate::document::probe_existing_style(&cmd.file);
    let mut disk_content = normalize::restore_line_endings(&write_content, line_ending);
    if !bom.is_empty() {
        disk_content.insert_str(0, &bom);
    }

    // 3. Write using fast_write or atomic_write
    if cmd.safe {
        crate::commands::common::atomic_write(&cmd.file, disk_content.as_bytes())?;
    } else {
        crate::commands::common::fast_write(&cmd.file, disk_content.as_bytes())?;
    }

    // 3b. Verify the write actually landed: the target must now be a
    //     regular file with the expected length. Without this, writes to
    //     device paths (e.g. `d\NUL` on Windows) report OK while creating
    //     nothing.
    crate::commands::common::verify_written_file(&cmd.file, disk_content.len())?;

    // 3c. On Windows the OS silently normalizes trailing dots/spaces and
    //     casing in file names (`literal.` writes `literal`). Surface a
    //     warning so agents are not surprised by the alias.
    #[cfg(windows)]
    warn_if_name_normalized(ctx, &cmd.file)?;

    // 4. Compute hash and line count from the written content (canonical path).
    let file_hash = hash::compute_file_hash(&write_content);
    let line_count = write_content.lines().count();

    // 5. Render output based on output mode.
    match ctx.output_mode() {
        crate::context::OutputMode::Compact => {
            // Agent-native: status line only, no re-read
            writeln!(
                ctx.stdout(),
                "OK {}#{} lines={}",
                crate::output::escape_compact_field(&cmd.file.display().to_string()),
                file_hash,
                line_count,
            )?;
        }
        crate::context::OutputMode::Verbose => {
            // Human-readable: re-read and dump full file (old default)
            let fc = crate::document::FileContent::load(&cmd.file)?;
            let raw_lines = fc.lines();
            let entries = fc.lines_with_hashes();
            writeln!(ctx.stdout(), "[{}#{}]", fc.path.display(), fc.hash)?;
            let mut hash_buf = [0u8; 2];
            for (i, entry) in entries.iter().enumerate() {
                if entry.content.is_empty() && i == raw_lines.len() - 1 && fc.trailing_newline {
                    continue;
                }
                hash::write_short_hash_bytes(&mut hash_buf, entry.short_hash);
                let hash_str = unsafe { std::str::from_utf8_unchecked(&hash_buf) };
                writeln!(ctx.stdout(), "{}:{}|{}", i + 1, hash_str, entry.content)?;
            }
        }
        crate::context::OutputMode::Json => {
            // Structured JSON
            let fc = crate::document::FileContent::load(&cmd.file)?;
            let raw_lines = fc.lines();
            let entries = fc.lines_with_hashes();
            let lines: Vec<serde_json::Value> = entries
                .iter()
                .enumerate()
                .filter(|(i, entry)| {
                    !(entry.content.is_empty() && *i == raw_lines.len() - 1 && fc.trailing_newline)
                })
                .map(|(i, entry)| {
                    serde_json::json!({
                        "n": i + 1,
                        "hash": hash::format_short_hash(entry.short_hash),
                        "content": entry.content,
                    })
                })
                .collect();
            let output = serde_json::json!({
                "success": true,
                "path": fc.path.display().to_string(),
                "hash": fc.hash,
                "lines": lines,
            });
            writeln!(ctx.stdout(), "{}", serde_json::to_string(&output)?)?;
        }
        crate::context::OutputMode::Ndjson => {
            // Same as compact
            writeln!(
                ctx.stdout(),
                "OK {}#{} lines={}",
                crate::output::escape_compact_field(&cmd.file.display().to_string()),
                file_hash,
                line_count,
            )?;
        }
    }

    Ok(())
}

/// Warn on stderr when the OS normalized the requested file name —
/// trailing dots/spaces stripped or casing changed — so the path the
/// caller typed is not literally the path that was written.
#[cfg(windows)]
fn warn_if_name_normalized<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &std::path::Path,
) -> Result<(), HashlineError> {
    let Some(requested) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(());
    };
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return Ok(());
    };
    let Some(actual) = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    // Only warn when the names are equivalent modulo OS normalization
    // (trailing dots/spaces, case). A genuinely different name — e.g. a
    // symlink resolving to another file — is not a normalization.
    let normalize = |s: &str| s.trim_end_matches(['.', ' ']).to_lowercase();
    if requested != actual && normalize(&requested) == normalize(&actual) {
        writeln!(
            ctx.stderr(),
            "warning: path '{}' was normalized by the OS to '{}'",
            path.display(),
            canonical.display()
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::WriteCmd;
    use crate::context::{CommandContext, OutputMode};
    use std::fs;
    use std::path::PathBuf;

    fn write_cmd(file: PathBuf, content: &str, force: bool, safe: bool) -> WriteCmd {
        WriteCmd {
            file,
            content: content.to_string(),
            force,
            json: false,
            verbose: false,
            safe,
        }
    }

    fn run_write(
        file: PathBuf,
        content: &str,
        force: bool,
        safe: bool,
    ) -> (Result<(), HashlineError>, Vec<u8>, Vec<u8>) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let result = {
            let mut ctx = CommandContext::new(&mut out, &mut err, OutputMode::Compact);
            run(&mut ctx, write_cmd(file, content, force, safe))
        };
        (result, out, err)
    }

    #[test]
    fn write_new_file_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new.txt");

        let (result, out, _err) = run_write(file.clone(), "hello\n", false, false);
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello\n");
        assert!(String::from_utf8(out).unwrap().starts_with("OK "));
    }

    #[test]
    fn write_safe_mode_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("safe.txt");

        let (result, _out, _err) = run_write(file.clone(), "atomic\n", false, true);
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file).unwrap(), "atomic\n");
    }

    #[test]
    fn write_existing_without_force_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("exists.txt");
        fs::write(&file, "old").unwrap();

        let (result, _out, _err) = run_write(file.clone(), "new", false, false);
        assert!(matches!(result, Err(HashlineError::TargetExists { .. })));
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
    }

    #[test]
    fn write_existing_with_force_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("exists.txt");
        fs::write(&file, "old").unwrap();

        let (result, _out, _err) = run_write(file.clone(), "new", true, false);
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    }

    #[test]
    fn write_to_directory_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (result, _out, _err) = run_write(dir.path().to_path_buf(), "x", true, false);
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn write_nul_device_fails_instead_of_false_ok() {
        let dir = tempfile::tempdir().unwrap();
        // `d\NUL` opens the NUL device: the write "succeeds" but nothing
        // is created. hashline must reject it, not print OK.
        let nul = dir.path().join("NUL");

        for safe in [false, true] {
            let (result, out, _err) = run_write(nul.clone(), "hello", true, safe);
            assert!(
                matches!(result, Err(HashlineError::InvalidPath { .. })),
                "safe={safe}: expected InvalidPath, got {result:?}"
            );
            assert!(!String::from_utf8_lossy(&out).contains("OK "));
        }
    }

    #[cfg(windows)]
    #[test]
    fn write_reserved_device_names_fail() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["CON", "aux.txt", "com1", "LPT9.log"] {
            let file = dir.path().join(name);
            let (result, _out, _err) = run_write(file, "x", true, false);
            assert!(
                matches!(result, Err(HashlineError::InvalidPath { .. })),
                "{name}: expected InvalidPath, got {result:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn write_trailing_dot_warns_about_normalization() {
        let dir = tempfile::tempdir().unwrap();
        // Windows strips the trailing dot: `literal.` writes `literal`.
        let file = dir.path().join("literal.");

        let (result, _out, err) = run_write(file, "data", false, false);
        assert!(result.is_ok());
        let stderr = String::from_utf8(err).unwrap();
        assert!(
            stderr.contains("warning:") && stderr.contains("normalized"),
            "expected normalization warning, got: {stderr}"
        );
        // The content landed in the normalized file.
        assert_eq!(
            fs::read_to_string(dir.path().join("literal")).unwrap(),
            "data"
        );
    }

    // -- bd-eol-bom-u7h: write --force preserves existing file style --------

    #[test]
    fn write_force_preserves_crlf_style() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crlf.txt");
        fs::write(&file, "old\r\ncontent\r\n").unwrap();

        let (result, _out, _err) = run_write(file.clone(), "new\ntext", true, false);
        assert!(result.is_ok());
        // The existing file's CRLF style is matched, not forced to LF.
        assert_eq!(fs::read(&file).unwrap(), b"new\r\ntext");
    }

    #[test]
    fn write_force_preserves_bom_and_cr_style() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bom-cr.txt");
        fs::write(&file, "\u{FEFF}old\rdata\r").unwrap();

        let (result, _out, _err) = run_write(file.clone(), "x\ny", true, false);
        assert!(result.is_ok());
        assert_eq!(fs::read(&file).unwrap(), "\u{FEFF}x\ry".as_bytes());
    }

    #[test]
    fn write_force_on_lf_file_stays_lf() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lf.txt");
        fs::write(&file, "old\n").unwrap();

        let (result, _out, _err) = run_write(file.clone(), "new\n", true, false);
        assert!(result.is_ok());
        assert_eq!(fs::read(&file).unwrap(), b"new\n");
    }

    #[test]
    fn write_new_file_is_lf_without_bom() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fresh.txt");

        // Even though the supplied content carries CRLF + BOM, a new file
        // is written in canonical LF/no-BOM form.
        let (result, _out, _err) = run_write(file.clone(), "\u{FEFF}a\r\nb\r\n", false, false);
        assert!(result.is_ok());
        assert_eq!(fs::read(&file).unwrap(), b"a\nb\n");
    }

    #[test]
    fn write_force_reported_hash_matches_reload() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("h.txt");
        fs::write(&file, "old\r\n").unwrap();

        let (result, out, _err) = run_write(file.clone(), "a\nb\n", true, false);
        assert!(result.is_ok());
        let fc = crate::document::FileContent::load(&file).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(&format!("#{}", fc.hash)));
        assert_eq!(fc.hash, hash::compute_file_hash("a\nb\n"));
    }

    #[cfg(windows)]
    #[test]
    fn write_normal_name_has_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");

        let (result, _out, err) = run_write(file, "data", false, false);
        assert!(result.is_ok());
        assert!(String::from_utf8(err).unwrap().is_empty());
    }
}
