use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::ExplodeCmd;
use crate::context::CommandContext;
use crate::document::{Document, NewlineStyle};
use crate::error::LinehashError;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ExplodeCmd,
) -> Result<(), LinehashError> {
    let report = explode(&cmd.file, &cmd.out, cmd.force)?;
    output::write_success_line(
        ctx,
        &format!(
            "Exploded {} line files into {}.",
            report.file_count,
            report.out_dir.display()
        ),
    )?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExplodeReport {
    pub file_count: usize,
    pub out_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ExplodeMeta {
    source: String,
    newline: String,
    trailing_newline: bool,
    line_count: usize,
}

pub fn explode(source: &Path, out_dir: &Path, force: bool) -> Result<ExplodeReport, LinehashError> {
    prepare_output_dir(out_dir, force)?;
    let doc = Document::load(source)?;

    for (index, line) in doc.lines.iter().enumerate() {
        let path = out_dir.join(format_filename(index + 1, line.short_hash));
        fs::write(path, &line.content)?;
    }

    let meta = ExplodeMeta {
        source: source.display().to_string(),
        newline: newline_name(doc.newline).to_string(),
        trailing_newline: doc.trailing_newline,
        line_count: doc.lines.len(),
    };
    fs::write(
        out_dir.join(".meta.json"),
        serde_json::to_vec_pretty(&meta)?,
    )?;

    Ok(ExplodeReport {
        file_count: doc.lines.len(),
        out_dir: out_dir.to_path_buf(),
    })
}

fn prepare_output_dir(out_dir: &Path, force: bool) -> Result<(), LinehashError> {
    if out_dir.exists() {
        let mut entries = fs::read_dir(out_dir)?;
        if entries.next().is_some() {
            if !force {
                return Err(LinehashError::ExplodeTargetExists {
                    path: out_dir.display().to_string(),
                });
            }
            fs::remove_dir_all(out_dir)?;
        } else if force {
            fs::remove_dir_all(out_dir)?;
        }
    }

    fs::create_dir_all(out_dir)?;
    Ok(())
}

fn format_filename(line_no: usize, short_hash: crate::hash::ShortHash) -> String {
    format!(
        "{line_no:04}_{}.txt",
        crate::document::format_short_hash(short_hash)
    )
}

fn newline_name(newline: NewlineStyle) -> &'static str {
    match newline {
        NewlineStyle::Lf => "lf",
        NewlineStyle::Crlf => "crlf",
    }
}

#[cfg(test)]
mod tests {
    use super::{ExplodeMeta, explode, format_filename};
    use crate::document::Document;
    use crate::error::LinehashError;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_explode_basic() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        let report = explode(&source, &out, false).unwrap();
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        assert_eq!(report.file_count, 2);
        assert_eq!(
            fs::read_to_string(out.join(format_filename(1, doc.lines[0].short_hash))).unwrap(),
            "alpha"
        );
        assert_eq!(
            fs::read_to_string(out.join(format_filename(2, doc.lines[1].short_hash))).unwrap(),
            "beta"
        );
    }

    #[test]
    fn test_explode_filename_format() {
        assert_eq!(format_filename(7, 0xa3), "0007_a3.txt");
        assert_eq!(format_filename(12345, 0xff), "12345_ff.txt");
    }

    #[test]
    fn test_explode_existing_dir_fails_without_force() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\n").unwrap();
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("keep.txt"), "keep").unwrap();

        let error = explode(&source, &out, false).unwrap_err();

        assert!(matches!(error, LinehashError::ExplodeTargetExists { .. }));
        assert_eq!(fs::read_to_string(out.join("keep.txt")).unwrap(), "keep");
    }

    #[test]
    fn test_explode_existing_dir_succeeds_with_force() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\n").unwrap();
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("stale.txt"), "stale").unwrap();

        let report = explode(&source, &out, true).unwrap();
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();

        assert_eq!(report.file_count, 1);
        assert!(!out.join("stale.txt").exists());
        assert!(
            out.join(format_filename(1, doc.lines[0].short_hash))
                .exists()
        );
    }

    #[test]
    fn test_explode_empty_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("empty.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "").unwrap();

        let report = explode(&source, &out, false).unwrap();
        let meta: ExplodeMeta =
            serde_json::from_slice(&fs::read(out.join(".meta.json")).unwrap()).unwrap();

        assert_eq!(report.file_count, 0);
        assert_eq!(meta.line_count, 0);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    }

    #[test]
    fn test_explode_meta_json_written() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\r\nbeta\r\n").unwrap();

        explode(&source, &out, false).unwrap();
        let meta: ExplodeMeta =
            serde_json::from_slice(&fs::read(out.join(".meta.json")).unwrap()).unwrap();

        assert_eq!(meta.source, source.display().to_string());
        assert_eq!(meta.newline, "crlf");
        assert!(meta.trailing_newline);
        assert_eq!(meta.line_count, 2);
    }

    #[test]
    fn test_explode_writes_no_newline_terminators() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        explode(&source, &out, false).unwrap();
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        assert_eq!(
            fs::read(out.join(format_filename(1, doc.lines[0].short_hash))).unwrap(),
            b"alpha"
        );
        assert_eq!(
            fs::read(out.join(format_filename(2, doc.lines[1].short_hash))).unwrap(),
            b"beta"
        );
    }

    #[test]
    fn test_known_hashes_match_fixture_names() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let out = dir.path().join("exploded");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        explode(&source, &out, false).unwrap();
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();

        let mut files = fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(
            files,
            vec![
                ".meta.json".to_string(),
                format_filename(1, doc.lines[0].short_hash),
                format_filename(2, doc.lines[1].short_hash),
            ]
        );
    }
}
