#![allow(dead_code)]

use std::collections::BTreeSet;
use std::io::{self, Write};

use serde::Serialize;

use crate::anchor::ResolvedLine;
use crate::context::{CommandContext, OutputMode};
use crate::document::{Document, FileStats, LineView, NewlineStyle, format_short_hash};
use crate::error::LinehashError;
use crate::orchestration::{IndexPayload, ReadPayload};
use crate::risk::blocked_assessment;

#[derive(Serialize)]
struct ErrorPayload<'a> {
    error: String,
    hint: Option<&'a str>,
    command: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    risk: Option<crate::risk::RiskAssessment>,
}

#[allow(dead_code)]
pub fn write_success_line<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    line: &str,
) -> io::Result<()> {
    writeln!(ctx.stdout(), "{line}")
}

#[allow(dead_code)]
pub fn write_json_success<W: Write, E: Write, T: Serialize + ?Sized>(
    ctx: &mut CommandContext<'_, W, E>,
    value: &T,
) -> io::Result<()> {
    serde_json::to_writer_pretty(ctx.stdout(), value)?;
    writeln!(ctx.stdout())
}

pub fn print_read(writer: &mut impl Write, doc: &Document) -> io::Result<()> {
    let width = line_number_width(doc);
    for (index, line) in doc.lines.iter().enumerate() {
        writeln!(
            writer,
            "{number:>width$}:{hash}| {content}",
            number = index + 1,
            hash = format_short_hash(line.short_hash),
            content = line.content,
            width = width
        )?;
    }
    Ok(())
}

pub fn print_read_json(writer: &mut impl Write, payload: &ReadPayload) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, &payload)?;
    writeln!(writer)
}

pub fn print_read_context(
    writer: &mut impl Write,
    doc: &Document,
    anchors: &[ResolvedLine],
    context: usize,
) -> io::Result<()> {
    let width = line_number_width(doc);
    let anchor_indexes: BTreeSet<usize> = anchors.iter().map(|anchor| anchor.index).collect();
    let included = collect_context_indexes(doc, anchors, context);

    let mut previous: Option<usize> = None;
    for index in included {
        if let Some(prev) = previous {
            if index > prev + 1 {
                writeln!(writer, "...")?;
            }
        }

        let marker = if anchor_indexes.contains(&index) {
            '→'
        } else {
            ' '
        };
        let line = &doc.lines[index];
        let number = (index + 1).to_string();
        let padding = width.saturating_sub(number.len());
        writeln!(
            writer,
            "{indent:>padding$}{marker}{number}:{hash}| {content}",
            indent = "",
            marker = marker,
            number = number,
            hash = format_short_hash(line.short_hash),
            content = line.content,
            padding = padding,
        )?;
        previous = Some(index);
    }

    Ok(())
}

pub fn print_index(writer: &mut impl Write, doc: &Document) -> io::Result<()> {
    for (index, line) in doc.lines.iter().enumerate() {
        writeln!(
            writer,
            "{}:{}",
            index + 1,
            format_short_hash(line.short_hash)
        )?;
    }
    Ok(())
}

pub fn print_index_json(writer: &mut impl Write, payload: &IndexPayload) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, &payload)?;
    writeln!(writer)
}

pub fn print_stats(writer: &mut impl Write, stats: &FileStats) -> io::Result<()> {
    writeln!(writer, "Lines: {}", stats.line_count)?;
    writeln!(writer, "Unique hashes (2-char): {}", stats.unique_hashes)?;
    writeln!(writer, "Collisions: {}", stats.collision_count)?;
    writeln!(writer, "Collision pairs: {}", stats.collision_pairs.len())?;
    writeln!(writer, "Est. read tokens: ~{}", stats.estimated_read_tokens)?;
    writeln!(
        writer,
        "Hash length advice: {}-char recommended",
        stats.hash_length_advice
    )?;
    writeln!(writer, "Suggested --context: {}", stats.suggested_context_n)?;
    writeln!(
        writer,
        "Recommended read mode: {}",
        stats.recommended_read_mode
    )?;
    writeln!(
        writer,
        "Recommended anchor mode: {}",
        stats.recommended_anchor_mode
    )?;
    writeln!(
        writer,
        "Recommended workflow: {}",
        stats.recommended_workflow
    )?;
    if stats.warnings.is_empty() {
        writeln!(writer, "Warnings: none")?;
    } else {
        writeln!(writer, "Warnings:")?;
        for warning in &stats.warnings {
            writeln!(writer, "- {warning}")?;
        }
    }
    writeln!(writer, "Note: v1 anchors still use fixed 2-char hashes.")
}

pub fn print_stats_json(writer: &mut impl Write, stats: &FileStats) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, stats)?;
    writeln!(writer)
}

pub fn print_grep(writer: &mut impl Write, doc: &Document, indexes: &[usize]) -> io::Result<()> {
    let width = line_number_width(doc);
    for index in indexes {
        let line = &doc.lines[*index];
        writeln!(
            writer,
            "{number:>width$}:{hash}| {content}",
            number = *index + 1,
            hash = format_short_hash(line.short_hash),
            content = line.content,
            width = width
        )?;
    }
    Ok(())
}

pub fn print_line_views(writer: &mut impl Write, lines: &[LineView]) -> io::Result<()> {
    let width = lines
        .iter()
        .map(|line| line.n.to_string().len())
        .max()
        .unwrap_or(1);
    for line in lines {
        writeln!(
            writer,
            "{number:>width$}:{hash}| {content}",
            number = line.n,
            hash = line.hash,
            content = line.content,
            width = width
        )?;
    }
    Ok(())
}

pub fn write_grep_json<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    lines: &[LineView],
) -> io::Result<()> {
    write_json_success(ctx, lines)
}

pub fn write_error<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    error: &LinehashError,
) -> io::Result<()> {
    match ctx.output_mode() {
        OutputMode::Pretty => {
            writeln!(ctx.stderr(), "Error: {error}")?;
            if let Some(risk) = blocked_assessment(error) {
                writeln!(
                    ctx.stderr(),
                    "Risk: {} - {}",
                    risk.level.as_str(),
                    risk.summary
                )?;
                for reason in risk.reasons {
                    writeln!(ctx.stderr(), "Reason: {}", reason.message)?;
                }
            }
            if let Some(hint) = error.hint() {
                writeln!(ctx.stderr(), "Hint: {hint}")?;
            }
            Ok(())
        }
        OutputMode::Json => {
            let risk = blocked_assessment(error);
            let payload = ErrorPayload {
                error: error.to_string(),
                hint: error.hint(),
                command: error.command().or_else(|| {
                    risk.as_ref().map(|risk| match risk.operation {
                        "patch" => "patch",
                        _ => "anchor-safety",
                    })
                }),
                risk,
            };
            serde_json::to_writer_pretty(ctx.stderr(), &payload)?;
            writeln!(ctx.stderr())
        }
    }
}

fn line_number_width(doc: &Document) -> usize {
    doc.lines.len().to_string().len().max(1)
}

fn newline_name(newline: NewlineStyle) -> &'static str {
    match newline {
        NewlineStyle::Lf => "lf",
        NewlineStyle::Crlf => "crlf",
    }
}

fn collect_context_indexes(doc: &Document, anchors: &[ResolvedLine], context: usize) -> Vec<usize> {
    let mut included = BTreeSet::new();

    for anchor in anchors {
        let start = anchor.index.saturating_sub(context);
        let end = (anchor.index + context).min(doc.lines.len().saturating_sub(1));
        for index in start..=end {
            included.insert(index);
        }
    }

    included.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{
        print_index, print_index_json, print_read, print_read_context, print_read_json,
        print_stats, print_stats_json,
    };
    use crate::anchor::ResolvedLine;
    use crate::document::{Document, FileStats, format_short_hash};
    use crate::orchestration::{index_payload, read_payload};
    use std::path::Path;

    #[test]
    fn test_read_format_single_line() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("1:{}| alpha\n", format_short_hash(doc.lines[0].short_hash))
        );
    }

    #[test]
    fn test_read_format_line_number_padding_2_digits() {
        let doc = numbered_doc(10);
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().next().unwrap().starts_with(" 1:"));
        assert!(rendered.lines().last().unwrap().starts_with("10:"));
    }

    #[test]
    fn test_read_format_line_number_padding_3_digits() {
        let doc = numbered_doc(100);
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().next().unwrap().starts_with("  1:"));
        assert!(rendered.lines().last().unwrap().starts_with("100:"));
    }

    #[test]
    fn test_read_context_marks_anchor_line() {
        let doc = numbered_doc(5);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[ResolvedLine {
                index: 2,
                line_no: 3,
                short_hash: format_short_hash(doc.lines[2].short_hash),
            }],
            1,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().any(|line| line.starts_with("→3:")));
    }

    #[test]
    fn test_read_context_suppresses_other_lines() {
        let doc = numbered_doc(5);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[ResolvedLine {
                index: 2,
                line_no: 3,
                short_hash: format_short_hash(doc.lines[2].short_hash),
            }],
            0,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered.lines().count(), 1);
        assert!(rendered.starts_with("→3:"));
    }

    #[test]
    fn test_read_context_multiple_anchors_merged() {
        let doc = numbered_doc(10);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[
                ResolvedLine {
                    index: 1,
                    line_no: 2,
                    short_hash: format_short_hash(doc.lines[1].short_hash),
                },
                ResolvedLine {
                    index: 8,
                    line_no: 9,
                    short_hash: format_short_hash(doc.lines[8].short_hash),
                },
            ],
            1,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("..."));
        assert!(
            rendered
                .lines()
                .any(|line| line.trim_start().starts_with("→2:"))
        );
        assert!(
            rendered
                .lines()
                .any(|line| line.trim_start().starts_with("→9:"))
        );
    }

    #[test]
    fn test_read_context_separator_between_neighborhoods() {
        let doc = numbered_doc(8);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[
                ResolvedLine {
                    index: 3,
                    line_no: 4,
                    short_hash: format_short_hash(doc.lines[3].short_hash),
                },
                ResolvedLine {
                    index: 4,
                    line_no: 5,
                    short_hash: format_short_hash(doc.lines[4].short_hash),
                },
            ],
            0,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered.lines().count(), 2);
        assert!(!rendered.contains("..."));
    }

    #[test]
    fn test_index_format_no_content() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let mut out = Vec::new();
        print_index(&mut out, &doc).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "1:{}\n2:{}\n",
                format_short_hash(doc.lines[0].short_hash),
                format_short_hash(doc.lines[1].short_hash)
            )
        );
    }

    #[test]
    fn test_stats_pretty_output_includes_summary_fields() {
        let stats = FileStats {
            line_count: 3,
            unique_hashes: 3,
            collision_count: 0,
            collision_pairs: vec![],
            estimated_read_tokens: 12,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "bare-or-qualified",
            recommended_workflow: "read -> annotate/grep -> verify -> edit/patch -> verify",
            warnings: vec![],
        };
        let mut out = Vec::new();
        print_stats(&mut out, &stats).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("Lines: 3"));
        assert!(rendered.contains("Unique hashes (2-char): 3"));
        assert!(rendered.contains("Hash length advice: 2-char recommended"));
        assert!(rendered.contains("Recommended read mode: read"));
        assert!(rendered.contains("Warnings: none"));
    }

    #[test]
    fn test_read_json_valid() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let mut out = Vec::new();
        let payload = read_payload(&doc, &[], 0).unwrap();
        print_read_json(&mut out, &payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["file"], "demo.txt");
        assert_eq!(parsed["newline"], "lf");
        assert_eq!(parsed["lines"][0]["content"], "alpha");
        assert_eq!(
            parsed["lines"][1]["hash"],
            format_short_hash(doc.lines[1].short_hash)
        );
    }

    #[test]
    fn test_index_json_valid() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let mut out = Vec::new();
        let payload = index_payload(&doc);
        print_index_json(&mut out, &payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["file"], "demo.txt");
        assert_eq!(parsed["lines"][0]["n"], 1);
        assert_eq!(
            parsed["lines"][0]["hash"],
            format_short_hash(doc.lines[0].short_hash)
        );
        assert!(parsed["lines"][0].get("content").is_none());
    }

    #[test]
    fn test_stats_json_valid() {
        let stats = FileStats {
            line_count: 1,
            unique_hashes: 1,
            collision_count: 0,
            collision_pairs: vec![],
            estimated_read_tokens: 2,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "bare-or-qualified",
            recommended_workflow: "read -> annotate/grep -> verify -> edit/patch -> verify",
            warnings: vec![],
        };
        let mut out = Vec::new();
        print_stats_json(&mut out, &stats).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["line_count"], 1);
        assert_eq!(parsed["hash_length_advice"], 2);
    }

    fn numbered_doc(count: usize) -> Document {
        let content = (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        Document::from_str(Path::new("demo.txt"), &content).unwrap()
    }
}
