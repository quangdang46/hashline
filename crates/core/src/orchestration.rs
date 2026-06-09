#![allow(unused)]

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::anchor::{ResolvedLine, parse_anchor, resolve};
use crate::cli::Commands;
use crate::document::{
    Document, FileStats, LineView, NewlineStyle, ShortHashIndex, build_index_from_counts,
    count_short_hashes, format_short_hash,
};
use crate::error::HashlineError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexLineView {
    pub n: usize,
    pub hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadPayload {
    pub file: String,
    pub newline: &'static str,
    pub trailing_newline: bool,
    pub mtime: i64,
    pub mtime_nanos: u32,
    pub inode: u64,
    pub lines: Vec<LineView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexPayload {
    pub file: String,
    pub lines: Vec<IndexLineView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifyResult {
    pub anchor: String,
    pub status: &'static str,
    pub line_no: Option<usize>,
    pub content: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifyReport {
    pub results: Vec<VerifyResult>,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorPayload {
    pub file: String,
    pub line_count: usize,
    pub estimated_read_tokens: usize,
    pub recommended_read_mode: &'static str,
    pub recommended_anchor_mode: &'static str,
    pub recommended_workflow: &'static str,
    pub suggested_context: usize,
    pub warnings: Vec<&'static str>,
    pub next_commands: Vec<String>,
}

pub fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Read(_) => "read",
        Commands::Index(_) => "index",
        Commands::Edit(_) => "edit",
        Commands::Insert(_) => "insert",
        Commands::Delete(_) => "delete",
        Commands::Verify(_) => "verify",
        Commands::Grep(_) => "grep",
        Commands::Annotate(_) => "annotate",
        Commands::Patch(_) => "patch",
        Commands::Swap(_) => "swap",
        Commands::Move(_) => "move",
        Commands::Indent(_) => "indent",
        Commands::Stats(_) => "stats",
        Commands::Doctor(_) => "doctor",
        Commands::FindBlock(_) => "find-block",
        Commands::Serve(_) => "serve",
        Commands::Mcp(_) => "mcp",
    }
}

/// Resolve anchors against `doc`.
///
/// Uses the cached short-hash index on `doc` when available; otherwise
/// builds a throwaway index for this call without mutating the document.
/// Callers that want the index cached should populate it first via
/// [`Document::build_index_cached`].
pub fn resolve_read_anchors(
    doc: &Document,
    anchors: &[String],
) -> Result<Vec<ResolvedLine>, HashlineError> {
    let owned: Option<ShortHashIndex> = if doc.short_hash_index.is_none() {
        let counts = count_short_hashes(&doc.lines);
        Some(build_index_from_counts(&doc.lines, &counts))
    } else {
        None
    };
    let index: &ShortHashIndex = match doc.short_hash_index.as_ref() {
        Some(cached) => cached,
        None => owned.as_ref().expect("owned index built when cache empty"),
    };
    anchors
        .iter()
        .map(|anchor| {
            let parsed = parse_anchor(anchor)?;
            resolve(&parsed, doc, index)
        })
        .collect()
}

pub fn read_payload(
    doc: &Document,
    anchors: &[String],
    context: usize,
) -> Result<ReadPayload, HashlineError> {
    let lines = if anchors.is_empty() {
        doc.lines
            .iter()
            .enumerate()
            .map(|(index, line)| LineView {
                n: index + 1,
                hash: format_short_hash(line.short_hash),
                content: line.content.to_string(),
            })
            .collect()
    } else {
        let resolved = resolve_read_anchors(doc, anchors)?;
        let indexes = collect_context_indexes(doc, &resolved, context);
        indexes
            .into_iter()
            .map(|index| {
                let line = &doc.lines[index];
                LineView {
                    n: index + 1,
                    hash: format_short_hash(line.short_hash),
                    content: line.content.to_string(),
                }
            })
            .collect()
    };

    Ok(ReadPayload {
        file: doc.path.display().to_string(),
        newline: newline_name(doc.newline),
        trailing_newline: doc.trailing_newline,
        mtime: doc
            .file_meta
            .as_ref()
            .map(|meta| meta.mtime_secs)
            .unwrap_or(0),
        mtime_nanos: doc
            .file_meta
            .as_ref()
            .map(|meta| meta.mtime_nanos)
            .unwrap_or(0),
        inode: doc.file_meta.as_ref().map(|meta| meta.inode).unwrap_or(0),
        lines,
    })
}

pub fn index_payload(doc: &Document) -> IndexPayload {
    IndexPayload {
        file: doc.path.display().to_string(),
        lines: doc
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| IndexLineView {
                n: index + 1,
                hash: format_short_hash(line.short_hash),
            })
            .collect(),
    }
}

pub fn verify_report(doc: &Document, anchors: &[String]) -> VerifyReport {
    let index = doc.build_index();
    let mut results = Vec::with_capacity(anchors.len());
    let mut has_failures = false;

    for anchor_str in anchors {
        match parse_anchor(anchor_str) {
            Ok(anchor) => match resolve(&anchor, doc, &index) {
                Ok(resolved) => results.push(VerifyResult {
                    anchor: anchor_str.clone(),
                    status: "ok",
                    line_no: Some(resolved.line_no),
                    content: Some(doc.lines[resolved.index].content.to_string()),
                    error: None,
                }),
                Err(error) => {
                    has_failures = true;
                    results.push(VerifyResult {
                        anchor: anchor_str.clone(),
                        status: verify_status_for_error(&error),
                        line_no: verify_line_no_for_error(&error),
                        content: None,
                        error: Some(error.to_string()),
                    });
                }
            },
            Err(error) => {
                has_failures = true;
                results.push(VerifyResult {
                    anchor: anchor_str.clone(),
                    status: verify_status_for_error(&error),
                    line_no: None,
                    content: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    VerifyReport {
        results,
        exit_code: i32::from(has_failures),
    }
}

pub fn doctor_payload(path: &Path, stats: &FileStats) -> DoctorPayload {
    let file = path.display().to_string();
    DoctorPayload {
        file: file.clone(),
        line_count: stats.line_count,
        estimated_read_tokens: stats.estimated_read_tokens,
        recommended_read_mode: stats.recommended_read_mode,
        recommended_anchor_mode: stats.recommended_anchor_mode,
        recommended_workflow: stats.recommended_workflow,
        suggested_context: stats.suggested_context_n,
        warnings: stats.warnings.clone(),
        next_commands: doctor_next_commands(&file, stats),
    }
}

fn verify_status_for_error(error: &HashlineError) -> &'static str {
    match error {
        HashlineError::HashNotFound { .. } => "not_found",
        HashlineError::AmbiguousHash { .. } => "ambiguous",
        HashlineError::StaleAnchor { .. } => "stale",
        HashlineError::InvalidAnchor { .. } => "parse_error",
        _ => "error",
    }
}

fn verify_line_no_for_error(error: &HashlineError) -> Option<usize> {
    match error {
        HashlineError::StaleAnchor { line, .. } => Some(*line),
        _ => None,
    }
}

fn doctor_next_commands(file: &str, stats: &FileStats) -> Vec<String> {
    let mut commands = Vec::new();

    if stats.recommended_read_mode == "read" {
        commands.push(format!("hashline read {file}"));
    } else {
        commands.push(format!("hashline index {file}"));
        commands.push(format!(
            "hashline read {file} --anchor <line:hash> --context {}",
            stats.suggested_context_n
        ));
    }

    commands.push(format!("hashline annotate {file} <text>"));
    commands.push(format!("hashline grep {file} <pattern>"));

    if stats.collision_count > 0 || stats.line_count > 2_000 {
        commands.push(format!("hashline find-block {file} <line:hash>"));
        commands.push(format!("hashline patch {file} <patch.json> --dry-run"));
    } else {
        commands.push(format!("hashline verify {file} <line:hash>"));
        commands.push(format!("hashline edit {file} <line:hash> <new_content>"));
    }

    commands
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

fn newline_name(newline: NewlineStyle) -> &'static str {
    match newline {
        NewlineStyle::Lf => "lf",
        NewlineStyle::Crlf => "crlf",
    }
}

// ---- run_command (moved from main.rs in 0.2.0 lib split) ----
//
// Top-level command dispatch. Used by:
//   - the bin (`hashline` CLI)
//   - the MCP server module (which translates JSON-RPC into Commands)
//   - any future library consumer that wants to drive the same dispatch
//     path with their own stdout/stderr writers.

use std::io::Write;

use crate::cli::Cli;
use crate::commands;
use crate::context::{CommandContext, json_pretty_for, output_mode_for};
use crate::risk::assess_command;
use tracing::{debug, info};

pub fn run<W: Write, E: Write>(
    cli: Cli,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<i32, HashlineError> {
    run_command(cli.command, stdout, stderr).map(|(code, _)| code)
}

/// Execute `command` and return `(exit_code, modified_doc)`.
///
/// `modified_doc` is `Some(doc)` when the command was a mutation that
/// modified a file (edit, insert, delete, patch, swap, move, indent).
/// Read-only commands return `None`.
pub fn run_command<W: Write, E: Write>(
    command: Commands,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(i32, Option<Document>), HashlineError> {
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
    let mut context =
        CommandContext::new(stdout, stderr, output_mode).with_json_pretty(json_pretty);

    let exit_code = match command {
        Commands::Read(cmd) => commands::read::run(&mut context, cmd).map(|_| 0),
        Commands::Index(cmd) => commands::index::run(&mut context, cmd).map(|_| 0),
        Commands::Edit(cmd) => commands::edit::run(&mut context, cmd).map(|_| 0),
        Commands::Insert(cmd) => commands::insert::run(&mut context, cmd).map(|_| 0),
        Commands::Delete(cmd) => commands::delete::run(&mut context, cmd).map(|_| 0),
        Commands::Verify(cmd) => commands::verify::run(&mut context, cmd),
        Commands::Grep(cmd) => commands::grep::run(&mut context, cmd).map(|_| 0),
        Commands::Annotate(cmd) => commands::annotate::run(&mut context, cmd).map(|_| 0),
        Commands::Patch(cmd) => commands::patch::run(&mut context, cmd).map(|_| 0),
        Commands::Swap(cmd) => commands::swap::run(&mut context, cmd).map(|_| 0),
        Commands::Move(cmd) => commands::r#move::run(&mut context, cmd).map(|_| 0),
        Commands::Indent(cmd) => commands::indent::run(&mut context, cmd).map(|_| 0),
        Commands::Stats(cmd) => commands::stats::run(&mut context, cmd).map(|_| 0),
        Commands::Doctor(cmd) => commands::doctor::run(&mut context, cmd).map(|_| 0),
        Commands::FindBlock(cmd) => commands::find_block::run(&mut context, cmd).map(|_| 0),
        Commands::Serve(cmd) => commands::serve::run(&mut context, cmd).map(|_| 0),
        Commands::Mcp(_) => unreachable!("mcp mode is handled before command dispatch"),
    }?;

    let modified_doc = context.modified_doc.take();
    Ok((exit_code, modified_doc))
}
#[cfg(test)]
mod tests {
    use super::{doctor_payload, index_payload, read_payload, verify_report};
    use crate::document::{Document, FileStats};
    use std::path::Path;

    #[test]
    fn read_payload_respects_anchor_context() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let anchor = format!("2:{}", super::format_short_hash(doc.lines[1].short_hash));
        let payload = read_payload(&doc, &[anchor], 1).unwrap();

        assert_eq!(payload.lines.len(), 3);
        assert_eq!(payload.lines[0].content, "alpha");
        assert_eq!(payload.lines[1].content, "beta");
        assert_eq!(payload.lines[2].content, "gamma");
    }

    #[test]
    fn verify_report_captures_success_and_failure() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let ok_anchor = format!("1:{}", super::format_short_hash(doc.lines[0].short_hash));
        let report = verify_report(&doc, &[ok_anchor, "bogus".into()]);

        assert_eq!(report.exit_code, 1);
        assert_eq!(report.results[0].status, "ok");
        assert_eq!(report.results[1].status, "parse_error");
    }

    #[test]
    fn doctor_payload_reuses_next_command_policy() {
        let stats = FileStats {
            line_count: 10,
            unique_hashes: 10,
            collision_count: 0,
            collision_pairs: vec![],
            collision_pair_count: 0,
            collision_pairs_truncated: false,
            estimated_read_tokens: 42,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "qualified",
            recommended_workflow: "read -> verify -> edit",
            warnings: vec!["demo warning"],
        };

        let payload = doctor_payload(Path::new("demo.txt"), &stats);
        assert_eq!(payload.file, "demo.txt");
        assert_eq!(payload.next_commands[0], "hashline read demo.txt");
        assert!(
            payload
                .next_commands
                .iter()
                .any(|command| command.contains("verify"))
        );
    }

    #[test]
    fn index_payload_keeps_path_and_hashes() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let payload = index_payload(&doc);
        assert_eq!(payload.file, "demo.txt");
        assert_eq!(payload.lines.len(), 1);
        assert_eq!(payload.lines[0].n, 1);
    }
}
