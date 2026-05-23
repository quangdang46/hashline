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
    Patch(PatchCmd),
    Swap(SwapCmd),
    Move(MoveCmd),
    Indent(IndentCmd),
    Stats(StatsCmd),
    Doctor(DoctorCmd),
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
}








#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run hashline as a JSON-RPC MCP server over stdio so agents can call the existing hashline feature set without shelling out."
)]
pub struct McpCmd {}






