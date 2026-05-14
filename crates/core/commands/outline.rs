use std::io::Write;

use crate::cli::OutlineCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::lang::detect::detect_language_from_path;
use crate::lang::outline::{MAX_OUTLINE_INPUT_BYTES, MAX_OUTLINE_INPUT_LINES, get_outline_entries};

/// Run the outline command to get structural outline of a file.
pub fn run<W, E>(ctx: &mut CommandContext<'_, W, E>, cmd: OutlineCmd) -> Result<(), LinehashError>
where
    W: Write,
    E: Write,
{
    let content = std::fs::read_to_string(&cmd.file).map_err(LinehashError::Io)?;

    let path_display = cmd.file.display().to_string();
    let byte_len = content.len();
    if byte_len > MAX_OUTLINE_INPUT_BYTES {
        return Err(LinehashError::OutlineInputTooLarge {
            path: path_display,
            actual: byte_len,
            limit: MAX_OUTLINE_INPUT_BYTES,
            unit: "bytes",
        });
    }

    // Count newlines as a fast proxy for line count (matches the way
    // `text.lines()` is used downstream). Cheaper than allocating a Vec.
    let line_count = content.as_bytes().iter().filter(|&&b| b == b'\n').count()
        + if content.ends_with('\n') || content.is_empty() {
            0
        } else {
            1
        };
    if line_count > MAX_OUTLINE_INPUT_LINES {
        return Err(LinehashError::OutlineInputTooLarge {
            path: path_display,
            actual: line_count,
            limit: MAX_OUTLINE_INPUT_LINES,
            unit: "lines",
        });
    }

    let lang = detect_language_from_path(&cmd.file);
    let entries = get_outline_entries(&content, lang);

    if cmd.json {
        let payload = serde_json::to_string_pretty(&entries).map_err(LinehashError::Json)?;
        writeln!(ctx.stdout(), "{}", payload).map_err(LinehashError::Io)?;
    } else {
        for entry in &entries {
            writeln!(
                ctx.stdout(),
                "{}:{:?}:{}",
                entry.start_line,
                entry.kind,
                entry.name
            )
            .map_err(LinehashError::Io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{CommandContext, OutputMode, SearchDocCache};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn run_with(file: PathBuf) -> Result<(Vec<u8>, Vec<u8>), LinehashError> {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let mut ctx = CommandContext::new(
            &mut stdout,
            &mut stderr,
            OutputMode::Pretty,
            SearchDocCache::new(1),
        );
        run(&mut ctx, OutlineCmd { file, json: false })?;
        drop(ctx);
        Ok((stdout, stderr))
    }

    #[test]
    fn outline_rejects_input_above_byte_limit() {
        // 5 MB + 1 byte of ASCII (one line, no newline). Triggers the byte guard
        // before the line guard.
        let mut tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("rs");
        let payload = vec![b'a'; MAX_OUTLINE_INPUT_BYTES + 1];
        std::io::Write::write_all(tmp.as_file_mut(), &payload).unwrap();
        std::fs::rename(tmp.path(), &path).unwrap();

        let err = run_with(path.clone()).unwrap_err();
        std::fs::remove_file(&path).ok();
        match err {
            LinehashError::OutlineInputTooLarge {
                actual,
                limit,
                unit,
                ..
            } => {
                assert_eq!(unit, "bytes");
                assert_eq!(limit, MAX_OUTLINE_INPUT_BYTES);
                assert_eq!(actual, MAX_OUTLINE_INPUT_BYTES + 1);
            }
            other => panic!("expected OutlineInputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn outline_rejects_input_above_line_limit() {
        // (MAX_OUTLINE_INPUT_LINES + 1) very short lines: stays well under the
        // byte limit but trips the line guard.
        let mut tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("rs");
        let mut payload = String::with_capacity(MAX_OUTLINE_INPUT_LINES * 2 + 2);
        for _ in 0..(MAX_OUTLINE_INPUT_LINES + 1) {
            payload.push_str("x\n");
        }
        std::io::Write::write_all(tmp.as_file_mut(), payload.as_bytes()).unwrap();
        std::fs::rename(tmp.path(), &path).unwrap();

        let err = run_with(path.clone()).unwrap_err();
        std::fs::remove_file(&path).ok();
        match err {
            LinehashError::OutlineInputTooLarge {
                actual,
                limit,
                unit,
                ..
            } => {
                assert_eq!(unit, "lines");
                assert_eq!(limit, MAX_OUTLINE_INPUT_LINES);
                assert_eq!(actual, MAX_OUTLINE_INPUT_LINES + 1);
            }
            other => panic!("expected OutlineInputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn outline_accepts_small_input() {
        // Tiny Rust file: should parse without hitting either guard.
        let mut tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("rs");
        let payload = "fn main() {\n    let x = 1;\n}\n";
        std::io::Write::write_all(tmp.as_file_mut(), payload.as_bytes()).unwrap();
        std::fs::rename(tmp.path(), &path).unwrap();

        let result = run_with(path.clone());
        std::fs::remove_file(&path).ok();
        let (stdout, _stderr) = result.unwrap();
        let out = String::from_utf8(stdout).unwrap();
        assert!(
            out.contains("main"),
            "expected `main` in outline, got {out:?}"
        );
    }
}
