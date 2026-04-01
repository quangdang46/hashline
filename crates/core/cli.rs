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
    Workflows(WorkflowsCmd),
    Explode(ExplodeCmd),
    Implode(ImplodeCmd),
    InstallMcp(InstallMcpCmd),
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
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct IndexCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
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
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
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
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
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
    #[serde(default)]
    #[arg(long)]
    pub invert: bool,
    #[serde(default)]
    #[arg(long)]
    pub case_insensitive: bool,
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
}

#[derive(Clone, Debug, Deserialize, Parser)]
pub struct FromDiffCmd {
    pub file: PathBuf,
    pub diff: String,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
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
    about = "Install MCP config into detected providers",
    long_about = "Detect local MCP host configs, upsert the linehash MCP server entry for each detected provider, and print install results."
)]
pub struct InstallMcpCmd {}

#[derive(Clone, Debug, Deserialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run linehash as a JSON-RPC MCP server over stdio so agents can call the existing linehash feature set without shelling out."
)]
pub struct McpCmd {}
