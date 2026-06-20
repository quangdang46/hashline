use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(
    name = "hashline",
    version,
    about = "Hash-anchored file editing for agents",
    long_about = "Hash-anchored file editing for agents. Typical workflow: read to inspect, then patch to apply edits."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Read(ReadCmd),
    Patch(PatchCmd),
    Write(WriteCmd),
    FindBlock(FindBlockCmd),
    Guide(GuideCmd),
    Serve(ServeCmd),
    Mcp(McpCmd),
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Read a file with [path#HASH] header + numbered lines")]
pub struct ReadCmd {
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Apply a hashline patch string to a file")]
pub struct PatchCmd {
    pub file: PathBuf,
    pub patch: String,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(
        long,
        help = "Use atomic temp-file + fsync (crash-safe but slower; default is fast direct write)"
    )]
    pub safe: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Find a structural block around an anchor")]
pub struct FindBlockCmd {
    pub file: PathBuf,
    pub anchor: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Write content to a file (creates new file or overwrites with --force)")]
pub struct WriteCmd {
    pub file: PathBuf,
    pub content: String,
    #[arg(long, help = "Overwrite existing file if it already exists")]
    pub force: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(
        long,
        help = "Use atomic temp-file + fsync (crash-safe but slower; default is fast direct write)"
    )]
    pub safe: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Replace old_string with new_string in file (str_replace-style)")]
pub struct ReplaceCmd {
    pub file: PathBuf,
    pub old_string: String,
    pub new_string: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub safe: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Show interactive user guide")]
pub struct GuideCmd {
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Run as a daemon over Unix socket or HTTP")]
pub struct ServeCmd {
    #[arg(long)]
    pub socket: Option<PathBuf>,
    #[arg(long)]
    pub http: Option<u16>,
    #[arg(long)]
    pub detach: bool,
    #[arg(long)]
    pub pid_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(about = "Run as an MCP server over stdio")]
pub struct McpCmd {
    #[arg(long)]
    pub proxy_to_daemon: bool,
}
