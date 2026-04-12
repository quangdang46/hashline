use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::ImplodeCmd;
use crate::commands::common::atomic_write;
use crate::context::CommandContext;
use crate::document::{Document, NewlineStyle};
use crate::error::LinehashError;
use crate::hash;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: ImplodeCmd,
) -> Result<(), LinehashError> {
    if cmd.dry_run {
        let plan = build_implode_plan(&cmd.dir)?;
        output::write_success_line(
            ctx,
            &format!(
                "Would implode {} line files into {}.",
                plan.line_count,
                cmd.out.display()
            ),
        )?;
        return output::write_success_line(ctx, "No file was written.")
            .map_err(LinehashError::from);
    }

    let report = implode(&cmd.dir, &cmd.out)?;
    output::write_success_line(
        ctx,
        &format!(
            "Imploded {} line files into {}.",
            report.line_count,
            report.out_file.display()
        ),
    )
    .map_err(LinehashError::from)
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImplodeReport {
    pub line_count: usize,
    pub out_file: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExplodeMeta {
    source: String,
    newline: String,
    trailing_newline: bool,
    line_count: usize,
}

struct ImplodePlan {
    line_count: usize,
    bytes: Vec<u8>,
}

pub fn implode(dir: &Path, out_file: &Path) -> Result<ImplodeReport, LinehashError> {
    let plan = build_implode_plan(dir)?;
    atomic_write(out_file, &plan.bytes)?;

    Ok(ImplodeReport {
        line_count: plan.line_count,
        out_file: out_file.to_path_buf(),
    })
}

fn build_implode_plan(dir: &Path) -> Result<ImplodePlan, LinehashError> {
    let meta_path = dir.join(".meta.json");
    let meta_bytes = fs::read(&meta_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LinehashError::ImplodeMissingMeta {
                path: dir.display().to_string(),
            }
        } else {
            LinehashError::Io(error)
        }
    })?;

    let meta: ExplodeMeta =
        serde_json::from_slice(&meta_bytes).map_err(|error| LinehashError::ImplodeInvalidMeta {
            path: meta_path.display().to_string(),
            reason: error.to_string(),
        })?;

    let newline = parse_newline(&meta, &meta_path)?;
    let mut lines = BTreeMap::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if name == ".meta.json" {
            continue;
        }

        if file_type.is_dir() {
            return Err(LinehashError::ImplodeDirtyDirectory {
                path: dir.display().to_string(),
                entry: name,
            });
        }

        let (line_no, expected_hash) = parse_line_filename(&name, dir)?;
        let bytes = fs::read(entry.path())?;
        let content = String::from_utf8(bytes).map_err(|_| LinehashError::InvalidUtf8 {
            path: entry.path().display().to_string(),
        })?;
        if content.contains(['\n', '\r']) {
            return Err(LinehashError::ImplodeInvalidMeta {
                path: entry.path().display().to_string(),
                reason: "line files must not contain newline terminators".to_owned(),
            });
        }

        let actual_hash = hash::short_hash(&content);
        if actual_hash != expected_hash {
            return Err(LinehashError::ImplodeInvalidMeta {
                path: entry.path().display().to_string(),
                reason: format!(
                    "filename encodes hash {expected_hash} but content hashes to {actual_hash}"
                ),
            });
        }

        if lines.insert(line_no, content).is_some() {
            return Err(LinehashError::ImplodeInvalidMeta {
                path: entry.path().display().to_string(),
                reason: format!("duplicate line file for line {line_no}"),
            });
        }
    }

    for line_no in 1..=meta.line_count {
        if !lines.contains_key(&line_no) {
            return Err(LinehashError::ImplodeMissingLineFile {
                path: dir.display().to_string(),
                line_no,
            });
        }
    }

    if lines.len() != meta.line_count {
        let unexpected = lines
            .keys()
            .find(|line_no| **line_no == 0 || **line_no > meta.line_count)
            .copied();
        if let Some(line_no) = unexpected {
            return Err(LinehashError::ImplodeInvalidMeta {
                path: dir.display().to_string(),
                reason: format!(
                    "line file {line_no} is outside metadata line_count {}",
                    meta.line_count
                ),
            });
        }
    }

    let rendered = render_document(
        newline,
        meta.trailing_newline,
        (1..=meta.line_count)
            .map(|line_no| lines.remove(&line_no).unwrap())
            .collect(),
    );

    let doc = Document::from_str(Path::new(&meta.source), &rendered)?;
    let bytes = doc.render();

    Ok(ImplodePlan {
        line_count: meta.line_count,
        bytes,
    })
}

fn parse_newline(meta: &ExplodeMeta, meta_path: &Path) -> Result<NewlineStyle, LinehashError> {
    match meta.newline.as_str() {
        "lf" => Ok(NewlineStyle::Lf),
        "crlf" => Ok(NewlineStyle::Crlf),
        other => Err(LinehashError::ImplodeInvalidMeta {
            path: meta_path.display().to_string(),
            reason: format!("unsupported newline style '{other}'"),
        }),
    }
}

fn parse_line_filename(name: &str, dir: &Path) -> Result<(usize, String), LinehashError> {
    let Some(stem) = name.strip_suffix(".txt") else {
        return Err(LinehashError::ImplodeDirtyDirectory {
            path: dir.display().to_string(),
            entry: name.to_owned(),
        });
    };
    let Some((line_no, short_hash)) = stem.split_once('_') else {
        return Err(LinehashError::ImplodeDirtyDirectory {
            path: dir.display().to_string(),
            entry: name.to_owned(),
        });
    };
    let line_no = line_no
        .parse::<usize>()
        .map_err(|_| LinehashError::ImplodeDirtyDirectory {
            path: dir.display().to_string(),
            entry: name.to_owned(),
        })?;
    let valid_hash = short_hash.len() == 2 && short_hash.chars().all(|ch| ch.is_ascii_hexdigit());
    if line_no == 0 || !valid_hash {
        return Err(LinehashError::ImplodeDirtyDirectory {
            path: dir.display().to_string(),
            entry: name.to_owned(),
        });
    }
    Ok((line_no, short_hash.to_ascii_lowercase()))
}

fn render_document(newline: NewlineStyle, trailing_newline: bool, lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let separator = match newline {
        NewlineStyle::Lf => "\n",
        NewlineStyle::Crlf => "\r\n",
    };
    let mut rendered = lines.join(separator);
    if trailing_newline {
        rendered.push_str(separator);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{ExplodeMeta, implode};
    use crate::commands::explode::explode;
    use crate::error::LinehashError;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_implode_round_trips_exploded_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::write(&source, "alpha\r\nbeta\r\n").unwrap();

        explode(&source, &exploded, false).unwrap();
        let report = implode(&exploded, &restored).unwrap();

        assert_eq!(report.line_count, 2);
        assert_eq!(fs::read(&restored).unwrap(), fs::read(&source).unwrap());
    }

    #[test]
    fn test_implode_missing_meta_fails() {
        let dir = TempDir::new().unwrap();
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::create_dir_all(&exploded).unwrap();

        let error = implode(&exploded, &restored).unwrap_err();
        assert!(matches!(error, LinehashError::ImplodeMissingMeta { .. }));
    }

    #[test]
    fn test_implode_rejects_unexpected_files() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        explode(&source, &exploded, false).unwrap();
        fs::write(exploded.join("notes.txt"), "oops").unwrap();

        let error = implode(&exploded, &restored).unwrap_err();
        assert!(matches!(error, LinehashError::ImplodeDirtyDirectory { .. }));
    }

    #[test]
    fn test_implode_rejects_missing_line_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        explode(&source, &exploded, false).unwrap();
        let second = fs::read_dir(&exploded)
            .unwrap()
            .find_map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("0002_").then_some(entry.path())
            })
            .unwrap();
        fs::remove_file(second).unwrap();

        let error = implode(&exploded, &restored).unwrap_err();
        assert!(matches!(
            error,
            LinehashError::ImplodeMissingLineFile { line_no: 2, .. }
        ));
    }

    #[test]
    fn test_implode_rejects_hash_mismatch() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("demo.txt");
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::write(&source, "alpha\nbeta\n").unwrap();

        explode(&source, &exploded, false).unwrap();
        let first = fs::read_dir(&exploded)
            .unwrap()
            .find_map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("0001_").then_some(entry.path())
            })
            .unwrap();
        fs::write(first, "changed").unwrap();

        let error = implode(&exploded, &restored).unwrap_err();
        assert!(matches!(error, LinehashError::ImplodeInvalidMeta { .. }));
        assert!(error.to_string().contains("filename encodes hash"));
    }

    #[test]
    fn test_implode_rejects_invalid_meta_newline() {
        let dir = TempDir::new().unwrap();
        let exploded = dir.path().join("exploded");
        let restored = dir.path().join("restored.txt");
        fs::create_dir_all(&exploded).unwrap();
        let meta = ExplodeMeta {
            source: "demo.txt".into(),
            newline: "weird".into(),
            trailing_newline: true,
            line_count: 0,
        };
        fs::write(
            exploded.join(".meta.json"),
            serde_json::to_vec_pretty(&meta).unwrap(),
        )
        .unwrap();

        let error = implode(&exploded, &restored).unwrap_err();
        assert!(matches!(error, LinehashError::ImplodeInvalidMeta { .. }));
        assert!(error.to_string().contains("unsupported newline style"));
    }
}
