use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Deserialize;

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
    Mcp(McpCmd),
}

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
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
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
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
}

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(clap::ValueEnum, Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveDirection {
    After,
    Before,
}

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
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

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run hashline as a JSON-RPC MCP server over stdio so agents can call the existing hashline feature set without shelling out."
)]
pub struct McpCmd {}
