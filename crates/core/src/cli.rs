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
    FindBlock(FindBlockCmd),
    Serve(ServeCmd),
    Mcp(McpCmd),
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Read a file with snapshot header",
    long_about = "Read a file and display it in oh-my-pi format: [path#HASH] header followed by numbered lines."
)]
pub struct ReadCmd {
    pub file: PathBuf,
    #[serde(default)]
    #[arg(long)]
    pub json: bool,
    /// Bypass any session cache and load the file fresh from disk.
    #[serde(default)]
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Apply a patch to a file",
    long_about = "Parse a hashline patch string and apply it to the target file. Supports SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL."
)]
pub struct PatchCmd {
    pub file: PathBuf,
    pub patch: String,
    #[serde(default)]
    #[arg(long)]
    pub dry_run: bool,
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

#[derive(Clone, Debug, Deserialize, Serialize, Parser)]
#[command(
    about = "Run as an MCP server over stdio",
    long_about = "Run hashline as a JSON-RPC MCP server over stdio so agents can call the existing hashline feature set without shelling out."
)]
pub struct McpCmd {
    /// Proxy MCP requests through to a running daemon via HASHLINE_SOCKET.
    #[arg(long)]
    pub proxy_to_daemon: bool,
}
