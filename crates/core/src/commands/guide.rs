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

    src/auth.js#1A2B
    1:ee|function verifyToken(token) {
    2:c6|  const decoded = jwt.verify(token, process.env.SECRET)
    3:a3|  if (!decoded.exp) throw new TokenError('missing expiry')
    4:18|  return decoded
    5:58|}

    Each line shows its xxh32 hash (e.g. 3:a3). Copy the anchor
    you want to target.

 2. PATCH to edit:

    $ hashline patch src/auth.js 'SWAP 3:a3:
    +  if (!decoded || !decoded.exp) throw TokenError("expired")'

    Output (compact, agent-first):
    OK src/auth.js#7f2a edits=1 changed=1
    ~3:b1|  if (!decoded || !decoded.exp) throw TokenError("expired")

    Use --verbose for full file dump after patch.
    Use --json for structured output with changed lines.

 3. FIND-BLOCK to locate structural context:

    $ hashline find-block src/auth.js 3:a3
    OK file=src/auth.js lang=JavaScript lines=5
    2:c6|  if (name) {
    3:db|    console.log("Hello " + name);
    4:8f|  }

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
  │ DEL N:HH:    │ Delete line N with hash validation           │
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
  ├──────────────┼──────────────────────────────────────────────┤
  │ CUT N..M     │ Capture lines N..M into a register and       │
  │ [@name]      │ delete them (anonymous when @name omitted)   │
  ├──────────────┼──────────────────────────────────────────────┤
  │ PUT [@name]  │ Paste a captured register before line N      │
  │ <N:          │ (bare PUT = file head)                       │
  └──────────────┴──────────────────────────────────────────────┘

───────────────────  RANGE SYNTAX  ──────────────────────────

  Two forms are accepted for ranges:
    A..B    hashline native range syntax
    A.=B    oh-my-pi compatible range syntax

───────────────────  ENVELOPE MARKERS  ──────────────────────

  Wrap patches in markers for embedding in text:

    *** Begin Patch
    [path/file#HASH]
    SWAP 5:
    +new content
    *** End Patch

  Use *** Abort to suppress a patch without applying it.

───────────────────  PATCH SOURCE MODES  ──────────────────────

  The patch argument accepts 3 forms:

    hashline patch file 'SWAP 3: +new'        Literal text (simple patches)
    hashline patch file - <<'EOF' ... EOF     Stdin — PREFERRED for multi-op
    hashline patch file @/path/to/file.patch  File reference (rare)

  ✅ PREFERRED — stdin via heredoc (no disk I/O):

    $ hashline patch src/auth.js - <<'EOF'
    *** Begin Patch
    SWAP 5:1a2b:
    +  const decoded = jwt.verify(token, env.SECRET)
    DEL 9
    SWAP 12:c3d4:
    +  return decoded
    *** End Patch
    EOF

  ❌ WRONG — do NOT create .patch files just for hashline:

    cat > /tmp/x.patch <<'EOF'       # waste: 2 extra I/O ops
    ...
    EOF
    hashline patch src/auth.js @/tmp/x.patch

  @path only when the patch file already exists (version-controlled
  patches, test fixtures, generated by another tool).

───────────────────  PAYLOAD ESCAPES  ──────────────────────

  +content    Normal payload line (content starts with text)
  ++content   Payload line starting with literal '+'
  +-content   Payload line starting with literal '-'

───────────────────  CONVENIENCE FLAGS  ──────────────────────

  Output modes (mutually exclusive):
    DEFAULT         Agent-native compact text (--verbose for human format)
    --verbose       Human-readable format (full file after mutations)
    --json          Structured JSON output

  Other flags:
  hashline patch <file> <patch> --dry-run    Preview only
  hashline patch <file> <patch> --safe       Atomic temp-file + fsync
  hashline read  <file>         --no-cache   Skip snapshot cache
  hashline find-block <f> <a>   --pretty     Pretty-print JSON (with --json)
  hashline write <file> <cont> --force       Overwrite existing file

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

  Runs a stdio JSON-RPC server exposing 6 tools:
    • read       — read file with snapshot hash
    • patch      — apply patch (SWAP/DEL/INS.*/BLK.*)
    • write      — write content to a new file
    • find_block — find enclosing syntactic block
    • remove_file — delete a file
    • rename_file — rename (move) a file

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
  OK config.rs#7f2a edits=1 changed=1
  ~10:b1|  pub const TIMEOUT: u64 = 5000;

  # Remove a function
  $ hashline patch lib.rs 'DEL.BLK 42'
  OK lib.rs#a1b2 edits=1 changed=5
  -42
  -43
  -44
  -45
  -46

  # Add import at top of file
  $ hashline patch main.rs 'INS.HEAD:
  +  use std::collections::HashMap;'
  OK main.rs#c3d4 edits=1 changed=1
  +1:9b|  use std::collections::HashMap;

  # Add debug log after line
  $ hashline patch handler.rs 'INS.POST 12:
  +  log::info!("request processed");'
  OK handler.rs#e5f6 edits=1 changed=1
  +13:a1|  log::info!("request processed");

  # Multi-op patch — use stdin to avoid creating a .patch file
  $ hashline patch model.rs - <<'EOF'
  *** Begin Patch
  SWAP 20:1a2b:
  +  pub fn new(name: String) -> Self {
  SWAP 25:c3d4:
  +      Self { name, enabled: true }
  +  }
  *** End Patch
  EOF
  OK model.rs#3e5a edits=2 changed=2
  ~20:f1|  pub fn new(name: String) -> Self {
  ~25:7c|      Self { name, enabled: true }

  # Human-readable output (full file after patch)
  $ hashline patch config.rs --verbose 'SWAP 10:
  +  pub const TIMEOUT: u64 = 5000;'

  # Structured JSON output
  $ hashline patch config.rs --json 'SWAP 10:
  +  pub const TIMEOUT: u64 = 5000;'
  {"success":true,"file":"config.rs","hash":"7f2a","edits_applied":1,"changed":[{"type":"modified","line":10,"hash":"b1","content":"  pub const TIMEOUT: u64 = 5000;"}]}

───────────────────────  TIPS  ───────────────────────────────

  • Default output is agent-first compact text (token-minimal)
  • Use --verbose for human-readable full file dump after mutations
  • Use --json for structured output with changed lines array
  • Always read before editing — anchors change when files change
  • Use --dry-run to preview patches before writing
  • An anchor like "42:a3" refers to line 42 with xxh32 hash a3
  • If an anchor fails, re-read the file for fresh hashes
  • For block ops, hashline auto-detects language by extension:
      .rs .js .ts .go .java → brace-balanced { }
      .py .verse            → indentation-based
      .rb                   → keyword (def/class/end)
  • hashline writes atomically (temp file + rename)
  • Use write to create new files: hashline write <file> <content>
  • Wrap complex patches in *** Begin/End Patch markers
  • Use ++ for literal +, +- for literal - in payload lines
  • A.=B range syntax is accepted alongside A..B
  • stdin (-) is always faster than @path — no file create/write overhead
  • Patch output prefix convention: ~modified +inserted -deleted
"#;

    writeln!(ctx.stdout(), "{}", guide.trim())?;
    Ok(())
}
