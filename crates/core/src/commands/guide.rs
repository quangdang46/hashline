use std::io::Write;

use crate::cli::GuideCmd;
use crate::context::CommandContext;
use crate::error::HashlineError;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    _cmd: GuideCmd,
) -> Result<(), HashlineError> {
    let guide = r#"
╔══════════════════════════════════════════════════════════════╗
║                     hashline user guide                      ║
║        Hash-anchored file editing for AI coding agents       ║
╚══════════════════════════════════════════════════════════════╝

────────────────────────────  BASICS  ────────────────────────────

 hashline uses xxh32 content hashes as stable anchors for
 line-level file editing. Anchors survive nearby edits because
 they reference content, not line numbers.

 ANCHOR FORMAT:    line:hash      e.g.  42:a3
                   line:hash..line:hash   42:a3..45:b7

────────────────────────  WORKFLOW  ────────────────────────────

 1. READ a file to see anchors:

    $ hashline read src/auth.js

    ──[src/auth.js#1A2B]────────────────────────────────────────
    1|   function verifyToken(token) {
    2|     const decoded = jwt.verify(token, process.env.SECRET)
    3:a3|   if (!decoded.exp) throw new TokenError('missing expiry')
    4|     return decoded
    5|   }

    Each line shows its xxh32 hash (e.g. 3:a3). Copy the anchor
    you want to target.

 2. PATCH to edit:

    $ hashline patch src/auth.js 'SWAP 3:a3:
    +  if (!decoded || !decoded.exp) throw TokenError("expired")'

    Or use positional range:

    $ hashline patch src/auth.js 'SWAP 3:a3..4:
    +  if (!decoded || !decoded.exp) throw TokenError("expired")
    +  return decoded'

 3. FIND-BLOCK to locate structural context:

    $ hashline find-block src/auth.js 3:a3

──────────────────────  PATCH OPERATIONS  ──────────────────────

  ┌──────────────┬──────────────────────────────────────────────┐
  │ SWAP N:      │ Replace line N with new content              │
  │ +content     │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ SWAP N..M:   │ Replace lines N through M                    │
  │ +c1          │                                              │
  │ +c2          │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ DEL N        │ Delete line N                                │
  ├──────────────┼──────────────────────────────────────────────┤
  │ DEL N..M     │ Delete lines N through M                     │
  ├──────────────┼──────────────────────────────────────────────┤
  │ INS.PRE N:   │ Insert content before line N                 │
  │ +content     │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ INS.POST N:  │ Insert content after line N                  │
  │ +content     │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ INS.HEAD:    │ Insert at start of file                      │
  │ +content     │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ INS.TAIL:    │ Insert at end of file                        │
  │ +content     │                                              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ SWAP.BLK N:  │ Replace entire syntactic block around N      │
  │ +content     │ (detected by braces, indentation, or Ruby)   │
  ├──────────────┼──────────────────────────────────────────────┤
  │ DEL.BLK N    │ Delete syntactic block around N              │
  ├──────────────┼──────────────────────────────────────────────┤
  │ INS.BLK.POST │ Insert after syntactic block around N        │
  │ N: +content  │                                              │
  └──────────────┴──────────────────────────────────────────────┘

───────────────────  CONVENIENCE FLAGS  ──────────────────────

  hashline patch <file> <patch> --dry-run    Preview only
  hashline patch <file> <patch> --json       JSON output
  hashline read  <file>         --json       JSON output
  hashline find-block <f> <a>   --json       JSON output
  hashline find-block <f> <a>   --pretty     Pretty-print JSON

──────────────────────  DAEMON MODE  ──────────────────────────

  # Start daemon:
  hashline serve --http 17300
  HASHLINE_URL=http://127.0.0.1:17300 hashline read src/file

  # Unix socket (default):
  hashline serve
  HASHLINE_SOCKET=~/.hashline/daemon.sock hashline read src/file

  # Detach to background:
  hashline serve --http 17300 --detach

────────────────────────  MCP MODE  ──────────────────────────

  hashline mcp

  Runs a stdio JSON-RPC server exposing 3 tools:
    • read       — read file with snapshot hash
    • patch      — apply patch (SWAP/DEL/INS.*/BLK.*)
    • find_block — find enclosing syntactic block

  Configure in claude_desktop_config.json / .cursor/mcp.json:
    {
      "mcpServers": {
        "hashline": { "command": "hashline", "args": ["mcp"] }
      }
    }

───────────────────────  EXAMPLES  ───────────────────────────

  # Replace text on a specific line
  $ hashline patch config.rs 'SWAP 10:
  +  pub const TIMEOUT: u64 = 5000;'

  # Remove a function
  $ hashline patch lib.rs 'DEL.BLK 42'

  # Add import at top of file
  $ hashline patch main.rs 'INS.HEAD:
  +  use std::collections::HashMap;'

  # Add debug log after line
  $ hashline patch handler.rs 'INS.POST 12:
  +  log::info!("request processed");'

  # Replace multiple lines
  $ hashline patch model.rs 'SWAP 20..25:
  +  pub fn new(name: String) -> Self {
  +      Self { name, enabled: true }
  +  }'

───────────────────────  TIPS  ───────────────────────────────

  • Always read before editing — anchors change when files change
  • Use --dry-run to preview patches before writing
  • An anchor like "42:a3" refers to line 42 with xxh32 hash a3
  • If an anchor fails, re-read the file for fresh hashes
  • For block ops, hashline auto-detects language by extension:
      .rs .js .ts .go .java → brace-balanced { }
      .py .verse            → indentation-based
      .rb                   → keyword (def/class/end)
  • hashline writes atomically (temp file + rename)
  • JSON mode is ideal for agent consumption
"#;

    writeln!(ctx.stdout(), "{}", guide.trim())?;
    Ok(())
}
