use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    Explode(ExplodeCmd),
    Implode(ImplodeCmd),
}

#[derive(Parser)]
#[command(
    about = "Read a file with line hashes",
    long_about = "Read a file with line hashes. Use full read for smaller files, or combine --anchor and --context to zoom in on a known target without dumping the entire file again."
)]
pub struct ReadCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub anchor: Vec<String>,
    #[arg(long, default_value = "5")]
    pub context: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct IndexCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct EditCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub content: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct InsertCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub content: String,
    #[arg(long)]
    pub before: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct DeleteCmd {
    pub file: PathBuf,
    pub anchor: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Check whether anchors still resolve",
    long_about = "Check whether anchors still resolve. Use verify before grouped edits or after locating anchors in files that may have changed."
)]
pub struct VerifyCmd {
    pub file: PathBuf,
    pub anchors: Vec<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Search file content and return anchor-addressed lines",
    long_about = "Search file content and return anchor-addressed lines. Use this when you know text or a regex pattern but still need current anchors before editing."
)]
pub struct GrepCmd {
    pub file: PathBuf,
    pub pattern: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub invert: bool,
    #[arg(long)]
    pub case_insensitive: bool,
}

#[derive(Parser)]
#[command(
    about = "Map text or regex matches back to current anchors",
    long_about = "Map text or regex matches back to current anchors. Prefer this when you know the target text and want the current line:hash anchor before verify/edit."
)]
pub struct AnnotateCmd {
    pub file: PathBuf,
    pub query: String,
    #[arg(long)]
    pub regex: bool,
    #[arg(long)]
    pub expect_one: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Apply a JSON patch transaction atomically",
    long_about = "Apply a JSON patch transaction atomically. Prefer patch when several related edits should succeed or fail together, or when you want a more reviewable multi-op workflow than many single-line commands."
)]
pub struct PatchCmd {
    pub file: PathBuf,
    pub patch: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct SwapCmd {
    pub file: PathBuf,
    pub anchor_a: String,
    pub anchor_b: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
}

#[derive(Parser)]
pub struct MoveCmd {
    pub file: PathBuf,
    pub anchor: String,
    pub direction: MoveDirection,
    pub target: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
}

#[derive(clap::ValueEnum, Clone, Copy)]
pub enum MoveDirection {
    After,
    Before,
}

#[derive(Parser)]
pub struct IndentCmd {
    pub file: PathBuf,
    pub range: String,
    #[arg(allow_hyphen_values = true)]
    pub amount: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub receipt: bool,
    #[arg(long)]
    pub audit_log: Option<PathBuf>,
    #[arg(long)]
    pub expect_mtime: Option<i64>,
    #[arg(long)]
    pub expect_inode: Option<u64>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Find a likely structural block around an anchor",
    long_about = "Find a likely structural block around an anchor. Use this before range edits, patch workflows, move/swap, or when a structural change is safer than multiple fragile line edits."
)]
pub struct FindBlockCmd {
    pub file: PathBuf,
    pub anchor: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Show file size, collision, and workflow guidance",
    long_about = "Show file size, collision, and workflow guidance. Use stats when a file is large, collisions are likely, or you want advice on whether to full-read, scope with anchors, or switch to patch-style edits."
)]
pub struct StatsCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
#[command(
    about = "Recommend a safe linehash workflow for a file",
    long_about = "Recommend a safe linehash workflow for a file. This is a read-only advisor that summarizes read strategy, anchor style, and when to prefer patch/find-block workflows on large or collision-heavy files."
)]
pub struct DoctorCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct FromDiffCmd {
    pub file: PathBuf,
    pub diff: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct MergePatchesCmd {
    pub patch_a: PathBuf,
    pub patch_b: PathBuf,
    #[arg(long)]
    pub base: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct WatchCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub continuous: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct ExplodeCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser)]
pub struct ImplodeCmd {
    pub dir: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub dry_run: bool,
}
