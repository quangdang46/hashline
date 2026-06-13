use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use crate::commands::batch::EditOp;

fn default_context() -> usize {
    5
}

#[derive(Parser)]
#[command(
    name = "hashline",
    version,
    about = "Hash-anchored file editing for agents",
    long_about = "Hash-anchored file editing for agents. Typical workflow: read or stats to inspect the file, verify anchors before grouped edits, then mutate with edit/insert/delete or patch."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Read(ReadCmd),
    Index(IndexCmd),
    Edit(EditCmd),
    Insert(InsertCmd),
    Delete(DeleteCmd),
    Verify(VerifyCmd),
    Grep(GrepCmd),
    Annotate(AnnotateCmd),
    Patch(PatchCmd),
    Swap(SwapCmd),
    Move(MoveCmd),
    Indent(IndentCmd),
    Stats(StatsCmd),
    Doctor(DoctorCmd),
    FindBlock(FindBlockCmd),
    ApplyDiff(DiffApplyCmd),
    Batch(BatchCmd),
    Serve(ServeCmd),
    Mcp(McpCmd),
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Read a file with line hashes",
    long_about = "Read a file with line hashes. Use full read for smaller files, or combine --anchor and --context to zoom in on a known target without dumping the entire file again."
)]
pub struct ReadCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub anchor: Vec<String>,
    #[serde(default = "default_context")]
    #[arg(long, default_value = "5")]
    pub context: usize,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Emit newline-delimited JSON (one object per line, no wrapper). Overrides --json.
    #[serde(default)]
    #[arg(long)]
    pub ndjson: bool,
    /// Bypass the session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct IndexCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Emit newline-delimited JSON (one object per line, no wrapper). Overrides --json.
    #[serde(default)]
    #[arg(long)]
    pub ndjson: bool,
    /// Bypass the session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct EditCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub content: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    /// Interpret C-style escape sequences in CONTENT (\n, \r, \t, \0, \\, \", \').
    /// Useful when the shell does not expand them. Defaults to literal content.
    #[serde(default)]
    #[arg(short = 'e', long)]
    pub interpret_escapes: bool,
    /// Stream the file line-by-line with BufReader instead of loading the
    /// entire Document into memory. Requires a qualified anchor (line:hash)
    /// and single-line content. No post-mutation cache is populated.
    /// Saves significant memory on files over 100k lines.
    #[serde(default)]
    #[arg(long)]
    pub streaming: bool,
    /// Content query to find the anchor line (mutually exclusive with anchor).
    #[serde(default)]
    #[arg(long)]
    pub start_query: Option<String>,
    /// Content query to find the end line (only with --start-query; for insert this determines placement).
    #[serde(default)]
    #[arg(long)]
    pub end_query: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct InsertCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub content: String,
    #[serde(default)]
    #[arg(long)]
    pub before: bool,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    /// Interpret C-style escape sequences in CONTENT (\n, \r, \t, \0, \\, \", \').
    /// Useful when the shell does not expand them. Defaults to literal content.
    #[serde(default)]
    #[arg(short = 'e', long)]
    pub interpret_escapes: bool,
    /// Content query to find the anchor line (mutually exclusive with anchor).
    #[serde(default)]
    #[arg(long)]
    pub start_query: Option<String>,
    /// Content query to find the end line (only with --start-query; for insert this determines placement).
    #[serde(default)]
    #[arg(long)]
    pub end_query: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct DeleteCmd {
    pub file: PathBuf,
    pub anchor: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
        /// Content query to find the start line of the target range (mutually exclusive with anchor).
    #[serde(default)]
    #[arg(long)]
    pub start_query: Option<String>,
    /// Content query to find the end line of the target range (only with --start-query).
    #[serde(default)]
    #[arg(long)]
    pub end_query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Check whether anchors still resolve",
    long_about = "Check whether anchors still resolve. Use verify before grouped edits or after locating anchors in files that may have changed."
)]
pub struct VerifyCmd {
    pub file: PathBuf,
    #[serde(default)]
    pub anchors: Vec<String>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Bypass the session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Search file content with a pattern",
    long_about = "Search file content and return matching lines with anchors. Supports literal and regex patterns. Use before hashline_read when you know a pattern and need to localize the target without dumping the whole file."
)]
pub struct GrepCmd {
    pub file: PathBuf,
    pub pattern: String,
    #[serde(default)]
    #[arg(short, long)]
    pub invert: bool,
    /// Case-insensitive search. Uses regex with (?i) prefix for correctness.
    #[serde(default)]
    #[arg(short = 'i', long)]
    pub case_insensitive: bool,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Emit newline-delimited JSON (one object per line, no wrapper). Overrides --json.
    #[serde(default)]
    #[arg(long)]
    pub ndjson: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Map text or regex matches back to current anchors",
    long_about = "Map text or regex matches back to current anchors. Searches file content and returns matching lines with line:hash|content format. Supports literal and regex queries. Use before hashline_read when you know the target text and want a precise anchor."
)]
pub struct AnnotateCmd {
    pub file: PathBuf,
    pub query: String,
    /// Treat query as a regex pattern.
    #[serde(default)]
    #[arg(short, long)]
    pub regex: bool,
    /// Require exactly one match; error if more or fewer.
    #[serde(default)]
    #[arg(short = '1', long)]
    pub expect_one: bool,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Emit newline-delimited JSON (one object per line, no wrapper). Overrides --json.
    #[serde(default)]
    #[arg(long)]
    pub ndjson: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Apply a JSON patch transaction atomically",
    long_about = "Apply a JSON patch transaction atomically. Prefer patch when several related edits should succeed or fail together, or when you want a more reviewable multi-op workflow than many single-line commands."
)]
pub struct PatchCmd {
    pub file: PathBuf,
    pub patch: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct SwapCmd {
    pub file: PathBuf,
    pub anchor_a: String,
    pub anchor_b: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct MoveCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub direction: MoveDirection,
    pub target: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveDirection {
    After,
    Before,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
pub struct IndentCmd {
    pub file: PathBuf,
    pub range: String,
    #[arg(allow_hyphen_values = true)]
    pub amount: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Show file size, collision, and workflow guidance",
    long_about = "Show file size, collision, and workflow guidance. Use stats when a file is large, collisions are likely, or you want advice on whether to full-read, scope with anchors, or switch to patch-style edits."
)]
pub struct StatsCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Bypass the session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Recommend a safe hashline workflow for a file",
    long_about = "Recommend a safe hashline workflow for a file. This is a read-only advisor that summarizes read strategy, anchor style, and when to prefer patch/find-block workflows on large or collision-heavy files."
)]
pub struct DoctorCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
    /// Bypass the session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Find a likely structural block around an anchor",
    long_about = "Given a line:hash anchor, detect the programming language from the file extension, then find the enclosing brace-delimited or indentation-based block and return it as a snippet with line:hash anchors."
)]
pub struct FindBlockCmd {
    pub file: PathBuf,
    pub anchor: String,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Apply a unified diff to a file",
    long_about = "Accept a unified diff content string (or read from stdin) and apply it to the target file. Each hunk is matched against current file content; unmatched hunks are reported as conflicts. Atomic: all hunks or none."
)]
pub struct DiffApplyCmd {
    pub file: PathBuf,
    /// Diff content as a string. If not provided, reads from stdin.
    #[serde(default)]
    #[arg(long)]
    pub diff: Option<String>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Apply multiple edits in a single atomic batch",
    long_about = "Apply multiple edits (replace, insert-after, delete, range) to the same file in a single read+hash+write pass. All anchors are validated before any mutation. Edits are applied bottom-up so line numbers remain stable. If any anchor is stale, the entire batch fails with no side effects."
)]
pub struct BatchCmd {
    pub file: PathBuf,
    /// JSON array of edit operations. Each op has `{type, anchor, content?}`.
    /// Types: "replace", "insertAfter", "delete", "range".
    #[serde(default, deserialize_with = "deserialize_edits")]
    #[arg(long, value_parser = parse_edits_json)]
    pub edits: Vec<EditOp>,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
    #[serde(default)]
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run hashline as a JSON-RPC MCP server over stdio so agents can call the existing hashline feature set without shelling out."
)]
pub struct McpCmd {
    /// Proxy MCP requests through to a running daemon via HASHLINE_SOCKET.
    /// The proxy forwards all JSON-RPC messages to the daemon socket and
    /// returns the daemon's responses, without maintaining its own session state.
    #[arg(long)]
    pub proxy_to_daemon: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Run as a daemon over a Unix socket or HTTP",
    long_about = "Run hashline as a background daemon listening on a Unix socket or HTTP port. \
    Set HASHLINE_SOCKET or HASHLINE_URL in your environment to route CLI commands through this daemon."
)]
pub struct ServeCmd {
    /// Unix socket path (e.g. /tmp/hashline.sock).
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// HTTP port to listen on (e.g. 17300).
    #[arg(long)]
    pub http: Option<u16>,
    /// Fork to background (Unix only).
    #[arg(long)]
    pub detach: bool,
    /// PID file path (default: ~/.hashline/daemon.pid).
    #[arg(long)]
    pub pid_file: Option<PathBuf>,
}

/// Deserialize `edits` from either a JSON array or a JSON string (which
/// itself contains a JSON array).  This lets the MCP server pass the edits
/// as a structured array while the CLI can accept them as `--edits '[...]'`.
pub fn deserialize_edits<'de, D>(deserializer: D) -> Result<Vec<EditOp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    // First try to deserialize as a json array
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Edits {
        Array(Vec<EditOp>),
        String(String),
    }
    let edits = Edits::deserialize(deserializer)?;
    match edits {
        Edits::Array(ops) => Ok(ops),
        Edits::String(s) => {
            serde_json::from_str(&s).map_err(de::Error::custom)
        }
    }
}

/// clap value parser: parse a JSON array string into `Vec<EditOp>`.
pub fn parse_edits_json(s: &str) -> Result<Vec<EditOp>, String> {
    serde_json::from_str(s).map_err(|e| e.to_string())
}
