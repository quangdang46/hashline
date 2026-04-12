#![allow(unused)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use memchr::memchr;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::anchor::{ResolvedLine, parse_anchor, resolve};
use crate::cli::Commands;
use crate::document::{Document, FileStats, LineView, NewlineStyle, format_short_hash};
use crate::error::LinehashError;
use crate::search::cache::SharedIndexCache;
use crate::search::filter::filter_candidates;
use crate::search::index::IndexBuilder;
use crate::search::verify::verify_candidates;

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
pub struct MatchReport {
    pub lines: Vec<LineView>,
    pub exit_code: i32,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WatchCapabilitiesPayload {
    pub cli_continuous_supported: bool,
    pub mcp_single_event_supported: bool,
    pub mcp_streaming_supported: bool,
    pub recommended_mcp_mode: &'static str,
    pub streaming_block_reason: &'static str,
    pub recommended_alternatives: Vec<&'static str>,
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
        Commands::FindBlock(_) => "find-block",
        Commands::Stats(_) => "stats",
        Commands::Doctor(_) => "doctor",
        Commands::Workflows(_) => "workflows",
        Commands::FromDiff(_) => "from-diff",
        Commands::MergePatches(_) => "merge-patches",
        Commands::Watch(_) => "watch",
        Commands::WatchCapabilities(_) => "watch-capabilities",
        Commands::Explode(_) => "explode",
        Commands::Implode(_) => "implode",
        Commands::InstallMcp(_) => "install-mcp",
        Commands::Mcp(_) => "mcp",
        Commands::Daemon => "daemon",
    }
}

pub fn watch_capabilities_payload() -> WatchCapabilitiesPayload {
    WatchCapabilitiesPayload {
        cli_continuous_supported: true,
        mcp_single_event_supported: true,
        mcp_streaming_supported: false,
        recommended_mcp_mode: "single-event watch",
        streaming_block_reason: "The linehash MCP server currently runs request/response stdio tools, so a continuous watch would block the caller instead of emitting incremental tool events.",
        recommended_alternatives: vec![
            "Call `linehash_watch` with `once=true` and re-issue it after each consumed event.",
            "Use CLI `linehash watch --continuous` outside MCP when you need a live terminal stream.",
            "Poll `linehash_read`, `linehash_index`, or `linehash_verify` when a client already owns the scheduling loop.",
        ],
    }
}

pub fn resolve_read_anchors(
    doc: &Document,
    anchors: &[String],
) -> Result<Vec<ResolvedLine>, LinehashError> {
    let index = doc.build_index();
    anchors
        .iter()
        .map(|anchor| {
            let parsed = parse_anchor(anchor)?;
            resolve(&parsed, doc, &index)
        })
        .collect()
}

pub fn read_payload(
    doc: &Document,
    anchors: &[String],
    context: usize,
) -> Result<ReadPayload, LinehashError> {
    let lines = if anchors.is_empty() {
        doc.lines
            .iter()
            .enumerate()
            .map(|(index, line)| LineView {
                n: index + 1,
                hash: format_short_hash(line.short_hash),
                content: line.content.clone(),
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
                    content: line.content.clone(),
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

pub fn grep_lines(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
) -> Result<Vec<LineView>, LinehashError> {
    if !case_insensitive && !contains_regex_metacharacters(pattern) {
        return grep_lines_fast(doc, pattern, invert);
    }

    let regex = RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|error| LinehashError::InvalidPattern {
            pattern: pattern.to_owned(),
            message: error.to_string(),
        })?;

    Ok(doc
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let is_match = regex.is_match(&line.content);
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: index + 1,
                hash: format_short_hash(line.short_hash),
                content: line.content.clone(),
            })
        })
        .collect())
}

fn grep_lines_fast(
    doc: &Document,
    pattern: &str,
    invert: bool,
) -> Result<Vec<LineView>, LinehashError> {
    let pattern_bytes = pattern.as_bytes();
    let mut results = Vec::new();

    if pattern_bytes.len() == 1 {
        let byte = pattern_bytes[0];
        for (index, line) in doc.lines.iter().enumerate() {
            let is_match = memchr(byte, line.content.as_bytes()).is_some();
            let include = if invert { !is_match } else { is_match };
            if include {
                results.push(LineView {
                    n: index + 1,
                    hash: format_short_hash(line.short_hash),
                    content: line.content.clone(),
                });
            }
        }
    } else {
        for (index, line) in doc.lines.iter().enumerate() {
            let is_match = if pattern_bytes.len() <= line.content.len() {
                line.content
                    .as_bytes()
                    .windows(pattern_bytes.len())
                    .any(|w| w == pattern_bytes)
            } else {
                false
            };
            let include = if invert { !is_match } else { is_match };
            if include {
                results.push(LineView {
                    n: index + 1,
                    hash: format_short_hash(line.short_hash),
                    content: line.content.clone(),
                });
            }
        }
    }

    Ok(results)
}

fn contains_regex_metacharacters(s: &str) -> bool {
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
            | '"' => return true,
            _ => {}
        }
    }
    false
}

pub fn grep_lines_indexed(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
) -> Result<Vec<LineView>, LinehashError> {
    let mut builder = IndexBuilder::new();
    for (idx, line) in doc.lines.iter().enumerate() {
        builder.add_line(idx, line.content.as_bytes());
    }
    let index = builder.build();

    let (candidates, is_match_all) = filter_candidates(&index, pattern);

    if is_match_all {
        return grep_lines(doc, pattern, invert, case_insensitive);
    }

    let results = verify_candidates(&candidates, &doc.lines, pattern, case_insensitive);

    let filtered: Vec<LineView> = results
        .into_iter()
        .filter_map(|r| {
            let is_match = true;
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: r.line_idx as usize + 1,
                hash: format_short_hash(doc.lines[r.line_idx as usize].short_hash),
                content: r.content.to_string(),
            })
        })
        .collect();

    Ok(filtered)
}

/// Grep with cached index for improved performance on repeated searches.
///
/// Uses a shared in-memory cache to avoid rebuilding the trigram index
/// on every search. The cache tracks file metadata (mtime, size, content hash)
/// to detect when the index needs rebuilding.
pub fn grep_lines_indexed_cached(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
    cache: &SharedIndexCache,
) -> Result<Vec<LineView>, LinehashError> {
    let mtime = doc
        .file_meta
        .as_ref()
        .map(|m| m.mtime_secs as u64)
        .unwrap_or(0);
    let content_bytes: Vec<u8> = doc
        .lines
        .iter()
        .flat_map(|l| l.content.as_bytes().to_vec())
        .collect();

    let index = cache
        .get_index(&doc.path, &content_bytes, mtime)
        .map_err(LinehashError::Io)?;

    let (candidates, is_match_all) = filter_candidates(&index, pattern);

    if is_match_all {
        return grep_lines(doc, pattern, invert, case_insensitive);
    }

    let results = verify_candidates(&candidates, &doc.lines, pattern, case_insensitive);

    let filtered: Vec<LineView> = results
        .into_iter()
        .filter_map(|r| {
            let is_match = true;
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: r.line_idx as usize + 1,
                hash: format_short_hash(doc.lines[r.line_idx as usize].short_hash),
                content: r.content.to_string(),
            })
        })
        .collect();

    Ok(filtered)
}

pub fn annotate_lines(
    doc: &Document,
    query: &str,
    regex: bool,
    expect_one: bool,
) -> Result<MatchReport, LinehashError> {
    let lines: Vec<LineView> = if regex {
        let regex =
            RegexBuilder::new(query)
                .build()
                .map_err(|error| LinehashError::InvalidPattern {
                    pattern: query.to_owned(),
                    message: error.to_string(),
                })?;

        doc.lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                regex.is_match(&line.content).then_some(LineView {
                    n: index + 1,
                    hash: format_short_hash(line.short_hash),
                    content: line.content.clone(),
                })
            })
            .collect()
    } else {
        doc.lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                line.content.contains(query).then_some(LineView {
                    n: index + 1,
                    hash: format_short_hash(line.short_hash),
                    content: line.content.clone(),
                })
            })
            .collect()
    };

    Ok(MatchReport {
        exit_code: i32::from(expect_one && lines.len() > 1),
        lines,
    })
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
                    content: Some(doc.lines[resolved.index].content.clone()),
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

fn verify_status_for_error(error: &LinehashError) -> &'static str {
    match error {
        LinehashError::HashNotFound { .. } => "not_found",
        LinehashError::AmbiguousHash { .. } => "ambiguous",
        LinehashError::StaleAnchor { .. } => "stale",
        LinehashError::InvalidAnchor { .. } => "parse_error",
        _ => "error",
    }
}

fn verify_line_no_for_error(error: &LinehashError) -> Option<usize> {
    match error {
        LinehashError::StaleAnchor { line, .. } => Some(*line),
        _ => None,
    }
}

fn doctor_next_commands(file: &str, stats: &FileStats) -> Vec<String> {
    let mut commands = Vec::new();

    if stats.recommended_read_mode == "read" {
        commands.push(format!("linehash read {file}"));
    } else {
        commands.push(format!("linehash index {file}"));
        commands.push(format!(
            "linehash read {file} --anchor <line:hash> --context {}",
            stats.suggested_context_n
        ));
    }

    commands.push(format!("linehash annotate {file} <text>"));
    commands.push(format!("linehash grep {file} <pattern>"));

    if stats.collision_count > 0 || stats.line_count > 2_000 {
        commands.push(format!("linehash find-block {file} <line:hash>"));
        commands.push(format!("linehash patch {file} <patch.json> --dry-run"));
    } else {
        commands.push(format!("linehash verify {file} <line:hash>"));
        commands.push(format!("linehash edit {file} <line:hash> <new_content>"));
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

#[cfg(test)]
mod tests {
    use super::{
        annotate_lines, doctor_payload, grep_lines, index_payload, read_payload, verify_report,
    };
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
    fn grep_lines_apply_case_insensitive_and_invert() {
        let doc = Document::from_str(Path::new("demo.txt"), "Alpha\nbeta\nGamma\n").unwrap();
        let matches = grep_lines(&doc, "alpha|gamma", false, true).unwrap();
        assert_eq!(matches.len(), 2);

        let inverted = grep_lines(&doc, "beta", true, false).unwrap();
        assert_eq!(inverted.len(), 2);
    }

    #[test]
    fn annotate_lines_reports_expect_one_exit_code() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\nalpha\n").unwrap();
        let report = annotate_lines(&doc, "alpha", false, true).unwrap();

        assert_eq!(report.exit_code, 1);
        assert_eq!(report.lines.len(), 2);
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
            estimated_read_tokens: 42,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "qualified",
            recommended_workflow: "read -> annotate -> verify -> edit",
            warnings: vec!["demo warning"],
        };

        let payload = doctor_payload(Path::new("demo.txt"), &stats);
        assert_eq!(payload.file, "demo.txt");
        assert_eq!(payload.next_commands[0], "linehash read demo.txt");
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
