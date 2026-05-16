use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use serde::Serialize;

use crate::cli::FromDiffCmd;
use crate::context::CommandContext;
use crate::document::Document;
use crate::error::LinehashError;
use crate::output::{JsonStyle, serialize_json};

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: FromDiffCmd,
) -> Result<(), LinehashError> {
    let doc = Document::load(&cmd.file)?;
    let diff = read_diff(&cmd.diff)?;
    let patch = compile_patch(&diff, &cmd.file, &doc)?;
    let style = JsonStyle::from_pretty(ctx.json_pretty());
    serialize_json(ctx.stdout(), &patch, style)?;
    Ok(())
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct PatchFile {
    file: String,
    ops: Vec<PatchOp>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
enum PatchOp {
    Edit {
        anchor: String,
        content: String,
    },
    Insert {
        anchor: String,
        content: String,
        #[serde(skip_serializing_if = "is_false")]
        before: bool,
    },
    Delete {
        anchor: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk {
    old_start: usize,
    lines: Vec<HunkLine>,
    header_line: usize,
}

fn read_diff(path: &str) -> Result<String, LinehashError> {
    if path == "-" {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        Ok(buffer)
    } else {
        Ok(fs::read_to_string(path)?)
    }
}

fn compile_patch(diff: &str, file_path: &Path, doc: &Document) -> Result<PatchFile, LinehashError> {
    let parsed = parse_diff(diff)?;
    validate_diff_target(parsed.target.as_deref(), file_path)?;

    let mut ops = Vec::new();
    for hunk in parsed.hunks {
        let start = locate_hunk(&hunk, doc).ok_or(LinehashError::DiffHunkMismatch {
            hunk_line: hunk.header_line,
        })?;
        compile_hunk(&mut ops, &hunk, start, doc)?;
    }

    Ok(PatchFile {
        file: file_path.display().to_string(),
        ops,
    })
}

#[derive(Default)]
struct ParsedDiff {
    target: Option<String>,
    hunks: Vec<Hunk>,
}

fn parse_diff(diff: &str) -> Result<ParsedDiff, LinehashError> {
    let mut parsed = ParsedDiff::default();
    let lines: Vec<&str> = diff.lines().collect();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if let Some(rest) = line.strip_prefix("+++ ") {
            parsed.target = Some(parse_diff_header_path(rest));
            index += 1;
            continue;
        }

        if let Some(hunk) = parse_hunk(&lines, &mut index)? {
            parsed.hunks.push(hunk);
            continue;
        }

        index += 1;
    }

    Ok(parsed)
}

fn parse_diff_header_path(header: &str) -> String {
    header
        .split_once('\t')
        .map(|(path, _)| path)
        .unwrap_or(header)
        .trim()
        .to_string()
}

fn parse_hunk(lines: &[&str], index: &mut usize) -> Result<Option<Hunk>, LinehashError> {
    let header = lines[*index];
    let Some(after_prefix) = header.strip_prefix("@@ -") else {
        return Ok(None);
    };

    let header_line = *index + 1;
    let old_range = after_prefix
        .split_whitespace()
        .next()
        .ok_or_else(|| invalid_diff(header_line, "missing hunk range"))?;
    let old_start = parse_hunk_start(old_range, header_line)?;

    *index += 1;
    let mut hunk_lines = Vec::new();
    while *index < lines.len() {
        let line = lines[*index];
        if line.starts_with("@@ -") {
            break;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            break;
        }
        if line == "\\ No newline at end of file" {
            *index += 1;
            continue;
        }
        let (prefix, content) = line.split_at(1);
        let parsed = match prefix {
            " " => HunkLine::Context(content.to_string()),
            "-" => HunkLine::Remove(content.to_string()),
            "+" => HunkLine::Add(content.to_string()),
            _ => {
                return Err(invalid_diff(
                    *index + 1,
                    "unexpected unified diff line prefix",
                ));
            }
        };
        hunk_lines.push(parsed);
        *index += 1;
    }

    Ok(Some(Hunk {
        old_start,
        lines: hunk_lines,
        header_line,
    }))
}

fn parse_hunk_start(range: &str, line_no: usize) -> Result<usize, LinehashError> {
    let start = range
        .split(',')
        .next()
        .ok_or_else(|| invalid_diff(line_no, "missing hunk start"))?;
    start
        .parse::<usize>()
        .map_err(|_| invalid_diff(line_no, "invalid hunk start"))
}

fn invalid_diff(line_no: usize, reason: &str) -> LinehashError {
    LinehashError::PatchFailed {
        op_index: 0,
        reason: format!("invalid unified diff at line {line_no}: {reason}"),
    }
}

fn validate_diff_target(target: Option<&str>, file_path: &Path) -> Result<(), LinehashError> {
    let Some(target) = target else {
        return Ok(());
    };
    let normalized_target = normalize_diff_path(target);
    let given = file_path.display().to_string();
    let normalized_given = normalize_fs_path(&given);
    if normalized_target == normalized_given
        || normalized_target.ends_with(&format!("/{normalized_given}"))
        || normalized_given.ends_with(&format!("/{normalized_target}"))
    {
        Ok(())
    } else {
        Err(LinehashError::DiffFileMismatch {
            diff_file: target.to_string(),
            given_file: given,
        })
    }
}

fn normalize_diff_path(path: &str) -> String {
    normalize_fs_path(path.trim_start_matches("a/").trim_start_matches("b/"))
}

fn normalize_fs_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn locate_hunk(hunk: &Hunk, doc: &Document) -> Option<usize> {
    let needle = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(text) | HunkLine::Remove(text) => Some(text.as_str()),
            HunkLine::Add(_) => None,
        })
        .collect::<Vec<_>>();

    if needle.is_empty() {
        return if doc.lines.is_empty() {
            Some(0)
        } else {
            Some((hunk.old_start.saturating_sub(1)).min(doc.lines.len()))
        };
    }

    let expected = hunk.old_start.saturating_sub(1);
    if matches_at(doc, expected, &needle) {
        return Some(expected);
    }

    let lower = expected.saturating_sub(10);
    let upper = (expected + 10).min(
        doc.lines
            .len()
            .saturating_sub(needle.len().saturating_sub(1)),
    );
    (lower..=upper).find(|start| matches_at(doc, *start, &needle))
}

fn matches_at(doc: &Document, start: usize, needle: &[&str]) -> bool {
    if start + needle.len() > doc.lines.len() {
        return false;
    }
    doc.lines[start..start + needle.len()]
        .iter()
        .map(|line| line.content.as_ref())
        .eq(needle.iter().copied())
}

fn compile_hunk(
    ops: &mut Vec<PatchOp>,
    hunk: &Hunk,
    start: usize,
    doc: &Document,
) -> Result<(), LinehashError> {
    let mut cursor = start;
    let mut index = 0;

    while index < hunk.lines.len() {
        match &hunk.lines[index] {
            HunkLine::Context(_) => {
                cursor += 1;
                index += 1;
            }
            HunkLine::Remove(_) | HunkLine::Add(_) => {
                let block_start = cursor;
                let mut removed = Vec::new();
                while index < hunk.lines.len() {
                    match &hunk.lines[index] {
                        HunkLine::Remove(text) => {
                            removed.push(text.clone());
                            cursor += 1;
                            index += 1;
                        }
                        _ => break,
                    }
                }

                let mut added = Vec::new();
                while index < hunk.lines.len() {
                    match &hunk.lines[index] {
                        HunkLine::Add(text) => {
                            added.push(text.clone());
                            index += 1;
                        }
                        _ => break,
                    }
                }

                emit_block(ops, doc, block_start, &removed, &added)?;
            }
        }
    }

    Ok(())
}

fn emit_block(
    ops: &mut Vec<PatchOp>,
    doc: &Document,
    start: usize,
    removed: &[String],
    added: &[String],
) -> Result<(), LinehashError> {
    let shared = removed.len().min(added.len());

    for (offset, content) in added.iter().enumerate().take(shared) {
        let line = doc_line(doc, start + offset)?;
        ops.push(PatchOp::Edit {
            anchor: format!(
                "{}:{}",
                start + offset + 1,
                crate::document::format_short_hash(line.short_hash)
            ),
            content: content.clone(),
        });
    }

    for offset in shared..removed.len() {
        let line = doc_line(doc, start + offset)?;
        ops.push(PatchOp::Delete {
            anchor: format!(
                "{}:{}",
                start + offset + 1,
                crate::document::format_short_hash(line.short_hash)
            ),
        });
    }

    if added.len() > shared {
        let extra = &added[shared..];
        if removed.is_empty() {
            if start < doc.lines.len() {
                let anchor_line = doc_line(doc, start)?;
                for content in extra.iter().rev() {
                    ops.push(PatchOp::Insert {
                        anchor: format!(
                            "{}:{}",
                            start + 1,
                            crate::document::format_short_hash(anchor_line.short_hash)
                        ),
                        content: content.clone(),
                        before: true,
                    });
                }
            } else if let Some(anchor_line) = doc.lines.last() {
                for content in extra {
                    ops.push(PatchOp::Insert {
                        anchor: format!(
                            "{}:{}",
                            doc.lines.len(),
                            crate::document::format_short_hash(anchor_line.short_hash)
                        ),
                        content: content.clone(),
                        before: false,
                    });
                }
            } else {
                return Err(LinehashError::DiffHunkMismatch { hunk_line: 0 });
            }
        } else {
            let anchor_line = doc_line(doc, start + removed.len() - 1)?;
            for content in extra {
                ops.push(PatchOp::Insert {
                    anchor: format!(
                        "{}:{}",
                        start + removed.len(),
                        crate::document::format_short_hash(anchor_line.short_hash)
                    ),
                    content: content.clone(),
                    before: false,
                });
            }
        }
    }

    Ok(())
}

fn doc_line(doc: &Document, index: usize) -> Result<&crate::document::LineRecord, LinehashError> {
    doc.lines
        .get(index)
        .ok_or(LinehashError::MutationIndexOutOfBounds {
            index,
            len: doc.lines.len(),
        })
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::{PatchOp, compile_patch};
    use crate::document::Document;
    use crate::error::LinehashError;
    use std::path::Path;

    #[test]
    fn compiles_simple_edit_hunk() {
        let doc = Document::from_str(Path::new("src/auth.js"), "alpha\nbeta\ngamma\n").unwrap();
        let patch = compile_patch(
            "+++ b/src/auth.js\n@@ -2,1 +2,1 @@\n-beta\n+BETA\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap();

        assert_eq!(patch.file, "src/auth.js");
        assert_eq!(patch.ops.len(), 1);
        assert_eq!(
            patch.ops[0],
            PatchOp::Edit {
                anchor: format!(
                    "2:{}",
                    crate::document::format_short_hash(doc.lines[1].short_hash)
                ),
                content: "BETA".into(),
            }
        );
    }

    #[test]
    fn compiles_insert_only_hunk_before_context() {
        let doc = Document::from_str(Path::new("src/auth.js"), "alpha\ngamma\n").unwrap();
        let patch = compile_patch(
            "+++ b/src/auth.js\n@@ -2,0 +2,1 @@\n+beta\n gamma\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap();

        assert_eq!(
            patch.ops,
            vec![PatchOp::Insert {
                anchor: format!(
                    "2:{}",
                    crate::document::format_short_hash(doc.lines[1].short_hash)
                ),
                content: "beta".into(),
                before: true,
            }]
        );
    }

    #[test]
    fn compiles_delete_only_hunk() {
        let doc = Document::from_str(Path::new("src/auth.js"), "alpha\nbeta\ngamma\n").unwrap();
        let patch = compile_patch(
            "+++ b/src/auth.js\n@@ -2,1 +2,0 @@\n-beta\n gamma\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap();

        assert_eq!(
            patch.ops,
            vec![PatchOp::Delete {
                anchor: format!(
                    "2:{}",
                    crate::document::format_short_hash(doc.lines[1].short_hash)
                ),
            }]
        );
    }

    #[test]
    fn compiles_mixed_hunk() {
        let doc =
            Document::from_str(Path::new("src/auth.js"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let patch = compile_patch(
            "+++ b/src/auth.js\n@@ -2,3 +2,3 @@\n-beta\n+BETA\n gamma\n-delta\n+between\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap();

        assert_eq!(patch.ops.len(), 2);
        assert_eq!(
            patch.ops[0],
            PatchOp::Edit {
                anchor: format!(
                    "2:{}",
                    crate::document::format_short_hash(doc.lines[1].short_hash)
                ),
                content: "BETA".into(),
            }
        );
        assert_eq!(
            patch.ops[1],
            PatchOp::Edit {
                anchor: format!(
                    "4:{}",
                    crate::document::format_short_hash(doc.lines[3].short_hash)
                ),
                content: "between".into(),
            }
        );
    }

    #[test]
    fn mismatched_hunk_fails() {
        let doc = Document::from_str(Path::new("src/auth.js"), "alpha\nbeta\ngamma\n").unwrap();
        let error = compile_patch(
            "+++ b/src/auth.js\n@@ -2,1 +2,1 @@\n-not-beta\n+BETA\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            LinehashError::DiffHunkMismatch { hunk_line: 2 }
        ));
    }

    #[test]
    fn file_mismatch_fails() {
        let doc = Document::from_str(Path::new("src/auth.js"), "alpha\n").unwrap();
        let error = compile_patch(
            "+++ b/src/other.js\n@@ -1,1 +1,1 @@\n-alpha\n+beta\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap_err();

        assert!(matches!(error, LinehashError::DiffFileMismatch { .. }));
    }

    #[test]
    fn relative_diff_target_matches_absolute_file_argument() {
        let doc = Document::from_str(Path::new("/tmp/work/demo.txt"), "alpha\n").unwrap();
        let patch = compile_patch(
            "+++ b/demo.txt\n@@ -1,1 +1,1 @@\n-alpha\n+beta\n",
            Path::new("/tmp/work/demo.txt"),
            &doc,
        )
        .unwrap();

        assert_eq!(patch.ops.len(), 1);
    }

    #[test]
    fn timestamped_unified_diff_target_matches_file_argument() {
        let doc = Document::from_str(Path::new("/tmp/work/demo.txt"), "alpha\n").unwrap();
        let patch = compile_patch(
            "+++ /tmp/work/demo.txt\t2026-03-29 21:43:38.307403626 +0700\n@@ -1,1 +1,1 @@\n-alpha\n+beta\n",
            Path::new("/tmp/work/demo.txt"),
            &doc,
        )
        .unwrap();

        assert_eq!(patch.ops.len(), 1);
    }

    #[test]
    fn fuzzy_match_allows_shifted_hunk() {
        let doc =
            Document::from_str(Path::new("src/auth.js"), "intro\nalpha\nbeta\ngamma\n").unwrap();
        let patch = compile_patch(
            "+++ b/src/auth.js\n@@ -1,1 +1,1 @@\n-alpha\n+ALPHA\n beta\n",
            Path::new("src/auth.js"),
            &doc,
        )
        .unwrap();

        assert_eq!(
            patch.ops[0],
            PatchOp::Edit {
                anchor: format!(
                    "2:{}",
                    crate::document::format_short_hash(doc.lines[1].short_hash)
                ),
                content: "ALPHA".into(),
            }
        );
    }
}
