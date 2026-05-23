use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Deserialize;

fn default_context() -> usize {
    5
}

#[derive(Parser)]
#[command(
    name = "linehash",
    version,
    about = "Hash-anchored file editing for agents",
    long_about = "Hash-anchored file editing for agents. Typical workflow: read or stats to inspect the file, annotate/grep to locate the target, verify anchors before grouped edits, then mutate with edit/insert/delete or patch."
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
    FindBlock(FindBlockCmd),
    Stats(StatsCmd),
    Doctor(DoctorCmd),
    FromDiff(FromDiffCmd),
    MergePatches(MergePatchesCmd),
    Watch(WatchCmd),
    WatchCapabilities(WatchCapabilitiesCmd),
    Workflows(WorkflowsCmd),
    Explode(ExplodeCmd),
    Implode(ImplodeCmd),
    Mcp(McpCmd),
    Daemon,
    Map(MapCmd),
    Outline(OutlineCmd),
    Symbol(SymbolCmd),
    Callers(CallersCmd),
    Callees(CalleesCmd),
    Deps(DepsCmd),
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
    about = "Search file content and return anchor-addressed lines",
    long_about = "Search file content and return anchor-addressed lines. Use this when you know text or a regex pattern but still need current anchors before editing."
)]
pub struct GrepCmd {
    pub file: PathBuf,
    pub pattern: String,
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
    #[serde(default)]
    #[arg(long)]
    pub invert: bool,
    #[serde(default)]
    #[arg(long)]
    pub case_insensitive: bool,
    #[serde(default)]
    #[arg(long)]
    pub no_index: bool,
    #[serde(default)]
    #[arg(long)]
    pub daemon: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Map text or regex matches back to current anchors",
    long_about = "Map text or regex matches back to current anchors. Prefer this when you know the target text and want the current line:hash anchor before verify/edit."
)]
pub struct AnnotateCmd {
    pub file: PathBuf,
    pub query: String,
    #[serde(default)]
    #[arg(long)]
    pub regex: bool,
    #[serde(default)]
    #[arg(long)]
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
    about = "Find a likely structural block around an anchor",
    long_about = "Find a likely structural block around an anchor. Use this before range edits, patch workflows, move/swap, or when a structural change is safer than multiple fragile line edits."
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
    about = "Recommend a safe linehash workflow for a file",
    long_about = "Recommend a safe linehash workflow for a file. This is a read-only advisor that summarizes read strategy, anchor style, and when to prefer patch/find-block workflows on large or collision-heavy files."
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
    about = "List repo-local markdown workflow packs",
    long_about = "List repo-local markdown workflow packs and skill docs from `.linehash/skills`. Use this to expose curated linehash agent workflows to both local CLI users and MCP clients without relying on ad hoc prompt text."
)]
pub struct WorkflowsCmd {
    #[arg(long)]
    pub root: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct FromDiffCmd {
    pub file: PathBuf,
    pub diff: String,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (defaults to compact).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct MergePatchesCmd {
    pub patch_a: PathBuf,
    pub patch_b: PathBuf,
    #[arg(long)]
    pub base: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct WatchCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub once: bool,
    #[serde(default)]
    #[arg(long)]
    pub continuous: bool,
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
    about = "Explain watch support across CLI and MCP surfaces",
    long_about = "Explain watch support across CLI and MCP surfaces. Use this before building an MCP client that expects streaming notifications so you can see the supported modes, the current MCP constraint, and the recommended fallback."
)]
pub struct WatchCapabilitiesCmd {
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct ExplodeCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct ImplodeCmd {
    pub dir: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run linehash as a JSON-RPC MCP server over stdio so agents can call the existing linehash feature set without shelling out."
)]
pub struct McpCmd {}

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Run as a persistent daemon for fast grep operations",
    long_about = "Start the linehash daemon that listens on a Unix socket and maintains an in-memory cache of file contents for sub-millisecond grep operations."
)]
pub struct DaemonCmd {}

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Map directory tree with token estimates",
    long_about = "Map a directory tree showing file structure with estimated token counts. Useful for understanding codebase size and structure before editing. Pass a positional PATH or use --scope; defaults to the current directory."
)]
pub struct MapCmd {
    /// Root directory to map. Defaults to current directory. Overridden by --scope if both are set.
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub scope: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub depth: Option<usize>,
    #[serde(default)]
    #[arg(long)]
    pub budget: Option<u64>,
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
    about = "Get structural outline of a file via tree-sitter",
    long_about = "Get structural outline of a file using tree-sitter parsing. Returns functions, classes, structs, and other definitions with their names and line numbers."
)]
pub struct OutlineCmd {
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
    about = "Search for symbol definitions and usages",
    long_about = "Search for symbol definitions and usages across files. Use this to find where a function, struct, or variable is defined and where it is used or called."
)]
pub struct SymbolCmd {
    pub query: String,
    #[serde(default)]
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub scope: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub expand: bool,
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
    about = "Find functions that call a given symbol",
    long_about = "Find functions that call a given symbol using BFS call graph traversal."
)]
pub struct CallersCmd {
    pub target: String,
    #[serde(default)]
    #[arg(long)]
    pub scope: Option<PathBuf>,
    #[arg(long, default_value = "3")]
    pub depth: usize,
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
    about = "Find functions called by a given symbol",
    long_about = "Find functions called by a given symbol using BFS call graph traversal."
)]
pub struct CalleesCmd {
    pub target: String,
    #[serde(default)]
    #[arg(long)]
    pub scope: Option<PathBuf>,
    #[arg(long, default_value = "3")]
    pub depth: usize,
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
    about = "Show dependency analysis for a file or directory",
    long_about = "Show imports and dependencies for a file or directory."
)]
pub struct DepsCmd {
    #[serde(default)]
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub scope: Option<PathBuf>,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON output (only takes effect with --json).
    #[serde(default)]
    #[arg(long)]
    pub pretty: bool,
}
