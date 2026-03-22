## why — Git History Archaeology CLI

`why` is a code-archaeology CLI that answers "why does this code exist?" by analyzing git history and synthesizing explanations via LLM. It provides risk assessment, ownership signals, and change context before you modify unfamiliar code.

### Why It's Useful

- **Prevents accidental deletions:** Understand why code was written before removing "dead-looking" functions
- **Risk-aware changes:** Get HIGH/MEDIUM/LOW risk signals based on commit history and keywords
- **Ownership discovery:** Find who knows the code and bus-factor risks with `--team`
- **Co-change awareness:** See coupled files before broad refactors with `--coupled`
- **Incident context:** Link code to past hotfixes, security patches, and incidents
- **LLM-powered synthesis:** One LLM call per query with structured git data as input

### Quick Start

```bash
# Basic query
why src/auth.rs:verify_token

# Validate config and test LLM connectivity
why doctor
```

### Query Syntax

| Form | Example | Description |
|------|---------|-------------|
| `<file>:<line>` | `why src/auth.rs:42` | Query specific line |
| `<file>:<symbol>` | `why src/auth.rs:verify_token` | Query function/method |
| `<file>:<Type::method>` | `why src/auth.rs:AuthService::login` | Qualified Rust method |
| `<file> --lines <start:end>` | `why src/auth.rs --lines 40:45` | Query line range |

### Command Reference

**Core Queries:**
| Command | Purpose |
|---------|---------|
| `why <target>` | Basic archaeology query with LLM synthesis |
| `why <target> --no-llm` | Heuristic-only mode (no LLM call) |
| `why <target> --json` | Machine-readable output |
| `why <target> --since 30` | Limit to recent 30 days |

**Risk & History:**
| Command | Purpose |
|---------|---------|
| `why <target> --blame-chain` | Walk past mechanical edits to true origin |
| `why <target> --evolution` | Rename-aware target history timeline |
| `why <target> --team` | Show ownership and bus-factor signals |
| `why <target> --coupled` | Show file-level co-change coupling |

**Code Actions:**
| Command | Purpose |
|---------|---------|
| `why <target> --annotate` | Write evidence-backed doc annotation |
| `why <target> --split` | Show archaeology-guided split suggestions |
| `why <target> --rename-safe` | Assess whether Rust symbol rename is safe |
| `why <target> --watch` | Refresh report when file changes |

**Repo-Wide Commands:**
| Command | Purpose |
|---------|---------|
| `why hotspots --limit 10` | Top churn × risk files |
| `why health` | Repo health dashboard |
| `why health --ci 80` | CI gate with threshold |
| `why time-bombs` | Aged TODOs and expired markers |
| `why ghost --limit 10` | Uncalled high-risk functions |
| `why onboard --limit 10` | Top symbols for new engineers |
| `why diff-review --no-llm` | Review staged diff |
| `why pr-template` | Generate PR template from staged diff |

**Config & Diagnostics:**
| Command | Purpose |
|---------|---------|
| `why config init` | Interactive setup (main entry point) |
| `why config init --local` | Create repo-local config |
| `why config get` | Show current effective config |
| `why doctor` | Validate config and test LLM |
| `why doctor --json` | Machine-readable diagnostics |

### Typical Agent Workflow

1. **Before deleting or refactoring unfamiliar code:**
   ```bash
   why src/legacy.rs:old_function --no-llm
   ```
   Check risk level and history before touching.

2. **For broader refactors:**
   ```bash
   why src/auth.rs:verify_token --coupled --team
   ```
   See coupled files and ownership before planning scope.

3. **For rename operations:**
   ```bash
   why src/auth.rs:verify_token --rename-safe
   ```
   Check caller risk to assess rename safety.

4. **Before PR:**
   ```bash
   why diff-review --no-llm
   why pr-template
   ```
   Review staged changes and generate template.

5. **Health check:**
   ```bash
   why health --json
   why hotspots --limit 5
   ```
   Understand repo-wide debt signals.

### Risk Levels

| Level | Meaning |
|-------|---------|
| **HIGH** | Stop and investigate. Likely security, incident, or critical business logic. |
| **MEDIUM** | Review carefully. Migration, integration, or compatibility-sensitive code. |
| **LOW** | Routine code. Standard review practices apply. |

### Supported Languages

Symbol resolution works for: Rust (`.rs`), Go (`.go`), JavaScript (`.js`), TypeScript (`.ts`, `.tsx`), Java (`.java`), Python (`.py`)

### Cache Behavior

- Query results cached in `.why/cache.jsonl`
- Health snapshots stored in `.why/health.jsonl`
- Use `--no-cache` to bypass cache
- Cache keys include `HEAD` hash prefix for natural invalidation

### Common Pitfalls

- **"could not find repository"**: Run from inside a git repo, or use repo-wide commands like `why doctor` from anywhere
- **Ambiguous symbol**: Use qualified name like `Module::function` instead of just `function`
- **No LLM synthesis**: Check `why doctor` for config/auth issues; fallback to `--no-llm` for heuristic-only
- **Missing credentials**: Run `why config init` or set `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`/`ZAI_API_KEY`

### Rules for Agents

- **Always run `why` before deleting unfamiliar code** — it may be a security fix or incident response
- Treat `HIGH` risk output as a stop-and-investigate signal
- Use `--blame-chain` to find true origin, not just last mechanical edit
- Use `--coupled` and `--team` before broader refactors
- Use `--no-llm` in CI or when LLM is unavailable
- Use `why doctor` to diagnose config/auth issues

---
## linehash — Hash-Anchored File Editing

`linehash` is a file editing tool that uses content-hashed line anchors (`12:ab3f`) instead of fragile exact-text matching. It's designed for agent-driven editing where concurrent changes are expected and edit safety is critical.

### Why It's Useful

- **Stable anchors:** Uses `line:hash` format that survives nearby edits—line numbers shift but hashes stay valid
- **Concurrent-safe:** Detects stale anchors when content changed; fails explicitly instead of guessing
- **Audit trail:** Optional `--receipt` and `--audit-log` for tracking edit history
- **No merge conflicts:** Each edit is independent; no patch files that conflict
- **Works with any text:** Language-agnostic; no parsing required

### The Anchor Format

Anchors are `line_number:content_hash` pairs like `42:a3f2`:

- **line_number**: 1-based line number (for human readability)
- **content_hash**: First 4+ chars of SHA-256 of line content (for stability)

Example output from `linehash read`:
```
  1:a1b2  fn main() {
  2:c3d4      println!("hello");
  3:e5f6  }
```

### Command Reference

**Reading:**
| Command | Purpose |
|---------|---------|
| `linehash read <file>` | Show file with line:hash anchors |
| `linehash read <file> --anchor 42:a3f2` | Show context around specific anchor |
| `linehash read <file> --context 10` | Set context lines (default: 5) |
| `linehash index <file>` | Show just anchors, no content |

**Editing:**
| Command | Purpose |
|---------|---------|
| `linehash edit <file> <anchor> <content>` | Replace line at anchor |
| `linehash edit <file> <start>..<end> <content>` | Replace line range |
| `linehash insert <file> <anchor> <content>` | Insert after anchor |
| `linehash insert <file> <anchor> <content> --before` | Insert before anchor |
| `linehash delete <file> <anchor>` | Delete line at anchor |
| `linehash delete <file> <start>..<end>` | Delete line range |

**Searching:**
| Command | Purpose |
|---------|---------|
| `linehash grep <file> <pattern>` | Search with anchor output |
| `linehash grep <file> <pattern> --case-insensitive` | Case-insensitive search |
| `linehash annotate <file> <query>` | Find and annotate matching lines |
| `linehash annotate <file> <regex> --regex` | Regex search |
| `linehash find-block <file> <anchor>` | Find enclosing block (brace/indent) |

**Utilities:**
| Command | Purpose |
|---------|---------|
| `linehash verify <file>` | Verify file integrity |
| `linehash stats <file>` | File statistics |
| `linehash patch <file> <patch-file>` | Apply patch by anchors |
| `linehash swap <file> <anchor1> <anchor2>` | Swap two lines |
| `linehash move <file> <anchor> <target-anchor>` | Move line to new position |
| `linehash indent <file> <anchor> <levels>` | Adjust indentation |

**Advanced:**
| Command | Purpose |
|---------|---------|
| `linehash from-diff <diff-file>` | Convert diff to anchor edits |
| `linehash merge-patches <file> <patch1> <patch2>` | Merge multiple patches |
| `linehash watch <file>` | Watch file for changes |
| `linehash explode <file>` | Split file into per-line files |
| `linehash implode <file>` | Reassemble from per-line files |

### Typical Agent Workflow

1. **Read file with anchors:**
   ```bash
   linehash read src/main.rs
   ```

2. **Find specific content:**
   ```bash
   linehash grep src/main.rs "fn process" --json
   ```

3. **Apply targeted edit:**
   ```bash
   linehash edit src/main.rs 42:a3f2 "fn process_data(input: &str) -> Result<()> {"
   ```

4. **Verify change:**
   ```bash
   linehash read src/main.rs --anchor 42:a3f2
   ```

5. **If anchor is stale, re-read and retry:**
   ```bash
   linehash read src/main.rs  # Get fresh anchors
   linehash edit src/main.rs 42:new_hash "..."
   ```

### Range Edits

Replace multiple lines with range syntax:

```bash
# Replace lines 10-15
linehash edit src/main.rs 10:a1b2..15:c3d4 "new content\nspanning\nmultiple lines"

# Delete lines 20-25
linehash delete src/main.rs 20:e5f6..25:g7h8
```

### Safety Features

**Stale anchor detection:**
```
Error: anchor 42:a3f2 is stale (line content changed)
Hint: re-run `linehash read src/main.rs` to get fresh anchors
```

**Ambiguous anchor detection:**
```
Error: anchor 42:a3 matches multiple lines
Hint: use more hash characters: 42:a3f2e1
```

**Dry-run mode:**
```bash
linehash edit src/main.rs 42:a3f2 "new content" --dry-run
```

**Audit logging:**
```bash
linehash edit src/main.rs 42:a3f2 "new content" --receipt --audit-log edits.jsonl
```

### JSON Output

All commands support `--json` for machine-readable output:

```bash
linehash read src/main.rs --json
linehash grep src/main.rs "fn " --json
linehash edit src/main.rs 42:a3f2 "new" --json
```

### Common Pitfalls

- **Stale anchor:** Content changed since last read → re-run `linehash read`
- **Ambiguous anchor:** Hash too short → use more characters from original hash
- **Line shifted:** Nearby edits changed line numbers → hash still works, just re-read
- **File deleted:** Obviously fails → check file exists before editing
- **Binary file:** Only works on text files → don't use on binaries

### Rules for Agents

- **Always prefer `linehash` over `sed`/`awk`** for targeted line edits
- **Re-read before editing** if file may have changed (other agents, user edits)
- **Treat stale-anchor failures as safety signals**, not errors to bypass
- **Use `--dry-run` first** when editing critical files
- **Use `--json` output** for parsing in scripts
- **Never force an edit** when anchor is stale—always re-read and retry

### When NOT to Use linehash

- **Large insertions:** For adding many lines, use a heredoc or write the whole file
- **Whole-file rewrites:** Just use `Write` tool directly
- **Binary files:** linehash only works on text
- **Complex refactors:** Use tree-sitter based tools for AST-aware changes

---

## scope — Static Analysis Dependency & Architecture Engine

`scope` is a local static-analysis workspace that answers "what depends on what?" by maintaining a SQLite-backed dependency/symbol graph. It provides dependency queries, impact analysis, architecture enforcement, capability auditing, and health reporting — all from a persistent local index.

### Why It's Useful

- **Dependency awareness:** Know what a file imports and what imports it before editing
- **Impact analysis:** Estimate blast radius of body/signature/rename/delete changes
- **Architecture enforcement:** Layer rules, capability auditing, entry-point reachability
- **Symbol graph:** Track exports, calls, callers, and public surface across files
- **Health reporting:** Aggregate metrics, gate checks, snapshot diffs, and risk hotspots
- **MCP integration:** Same queries available as stdio MCP tools for external clients

### Quick Start

```bash
# Index the repository
scope index .

# Query dependencies
scope deps src/lib.rs
scope deps src/lib.rs --reverse

# Check health
scope report
scope doctor
```

### Command Reference

**Indexing:**
| Command | Purpose |
|---------|---------|
| `scope index [PATH]` | Scan and index all supported files into `.scope/index.db` |
| `scope doctor [--fix]` | Validate index health and diagnostics |
| `scope benchmark [--fixture ...] [--iterations N]` | Performance benchmark with report |

**Dependency Queries:**
| Command | Purpose |
|---------|---------|
| `scope deps <file>` | Forward dependencies of a file |
| `scope deps <file> --reverse` | Reverse dependencies (what imports this) |
| `scope deps <file> --transitive [--depth N]` | Transitive closure with optional depth limit |
| `scope symbols <file> [--public-only] [--kind ...]` | Symbols defined in a file |
| `scope calls <symbol> [--transitive]` | What a symbol calls |
| `scope callers <symbol> [--transitive]` | What calls a symbol |

**Impact & Traversal:**
| Command | Purpose |
|---------|---------|
| `scope impact <target> --change-type <type>` | Estimate blast radius (body/signature/rename/delete/visibility/side-effect) |
| `scope explain <target> [--to ...] [--depth N]` | Explain dependency path |
| `scope why <from> <to> [--depth N]` | Find shortest path between two nodes |
| `scope context --target <...> --change-type <...> [--budget N]` | Structured change-planning context |
| `scope pack <target> --change-type <...> --budget <N>` | Lean plain-text context handoff |

**Architecture & Audit:**
| Command | Purpose |
|---------|---------|
| `scope arch check` | Check layer rule violations |
| `scope audit --capability <name>` | Capability reach analysis (e.g., network access) |
| `scope surface [<file>]` | Public surface of a file |
| `scope surface_diff <before> <after>` | Diff public surface between two files |
| `scope entry_list / entry_cone / entry_reaches / entry_unreachable` | Entry point analysis |

**Graph Analysis:**
| Command | Purpose |
|---------|---------|
| `scope unused` | Find unused exported symbols |
| `scope cycles [--severity ...]` | Detect dependency cycles |
| `scope tree <target> [--reverse] [--depth N]` | Dependency tree view |
| `scope split <target> --clusters <N>` | Suggest file split clusters |
| `scope mirror <target> --other <file>` | Similarity analysis between files |
| `scope stability [--file ...] [--sort ...]` | File instability metrics |
| `scope risk [--file ...] [--days N] [--top N]` | Risk hotspot analysis |
| `scope cochange [--file ...] [--min-commits N]` | Co-change coupling from git history |

**Reporting & Gating:**
| Command | Purpose |
|---------|---------|
| `scope report [--compare <snapshot>]` | Full health report with metrics |
| `scope gate [--compare <snapshot>] [--strict]` | CI gate check against thresholds |
| `scope diff --branch <ref>` | Changed/affected files relative to branch |
| `scope snapshot_save --name <name>` | Save current graph state |
| `scope snapshot_list / snapshot_delete` | Manage snapshots |
| `scope diff_snapshot` | Compare two snapshots |

**Other:**
| Command | Purpose |
|---------|---------|
| `scope query --expr "<pipe-expr>"` | Ad-hoc query language |
| `scope simulate extract ...` | Simulate file extraction impact |
| `scope rename_plan <target> --to <new_name>` | Plan symbol rename with edit steps |
| `scope test_map_covers <file>` | Which tests cover a source file |
| `scope serve [--port 7777]` | Local HTTP API + embedded UI |

### MCP Tool Surface (scope-mcp)

The same queries are exposed as stdio MCP tools. Tool names match CLI subcommands:

`index`, `deps`, `symbols`, `calls`, `callers`, `impact`, `explain`, `why`, `context`, `pack`, `arch_check`, `audit`, `stability`, `risk`, `cochange`, `report`, `gate`, `query`, `surface`, `surface_diff`, `test_map_covers`, `rename_plan`, `unused`, `cycles`, `diff`, `tree`, `split`, `mirror`, `entry_list`, `entry_cone`, `entry_reaches`, `entry_unreachable`, `doctor`, `benchmark`, `snapshot_save`, `snapshot_list`, `snapshot_delete`, `diff_snapshot`, `simulate_extract`

All MCP tools accept `repo_root` (optional, defaults to cwd) and return the same JSON envelope as CLI.

### Typical Agent Workflow

1. **Index once per session:**
   ```bash
   scope index .
   ```

2. **Before editing a file — check dependencies:**
   ```bash
   scope deps src/parser.rs
   scope deps src/parser.rs --reverse
   scope --compact symbols src/parser.rs --public-only
   ```

3. **Before refactoring — estimate blast radius:**
   ```bash
   scope impact src/parser.rs --change-type signature
   scope --compact callers parser::parse
   scope context --target parser::parse --change-type body --budget 400
   ```

4. **Before PR — health check:**
   ```bash
   scope report
   scope gate --strict
   scope unused
   scope cycles
   ```

### Output Format

All commands return a stable JSON envelope:

```json
{
  "schema_version": 1,
  "command": "deps",
  "status": "ok",
  "data": { ... },
  "warnings": []
}
```

- `status`: `ok`, `stub`, or `error`
- Use `--compact` for minified JSON (reduces tokens in agent loops)

### Architecture Config (`.scope/arch.toml`)

```toml
[[layer]]
name = "services"
pattern = "src/services/**"

[[rule]]
from = "models"
may_not_import = ["services", "routes"]
message = "models must not import services or routes"

[[capability]]
name = "network"
pattern = "src/http/**"
expected_callers = ["src/workers/**"]

[[entry_point]]
pattern = "src/cli/**"
```

### Supported Languages

- **Full semantic extraction:** Rust (`.rs`), TypeScript (`.ts`, `.tsx`), JavaScript (`.js`)
- **Scan-only (indexed but not deeply analyzed):** Python (`.py`), Ruby (`.rb`), Go (`.go`)

### Common Pitfalls

- **Stale index:** Run `scope index .` after significant code changes
- **Missing `.scope/`:** The `.scope/` directory is gitignored by default; each clone needs a fresh `scope index`
- **Heuristic resolution:** Results are static approximations; treat as evidence, not proof
- **Dynamic imports:** Computed module paths and reflection are blind spots
- **TypeScript paths:** `tsconfig` path mapping not yet fully supported

### Rules for Agents

- **Always `scope index .` before querying** if the index might be stale
- Use `--compact` in automated loops to save tokens
- Treat results as static evidence with explicit certainty levels (`exact` > `resolved` > `heuristic` > `dynamic`)
- Use `scope deps --reverse` before deleting files to find dependents
- Use `scope impact` before signature/rename changes to estimate blast radius
- Use `scope report` / `scope gate` for pre-PR health validation
- **Never treat scope output as proof a change is safe** — always verify with tests and builds


---

## MCP Agent Mail — Multi-Agent Coordination

A mail-like layer that lets coding agents coordinate asynchronously via MCP tools and resources. Provides identities, inbox/outbox, searchable threads, and advisory file reservations with human-auditable artifacts in Git.

### Why It's Useful

- **Prevents conflicts:** Explicit file reservations (leases) for files/globs
- **Token-efficient:** Messages stored in per-project archive, not in context
- **Quick reads:** `resource://inbox/...`, `resource://thread/...`

### Same Repository Workflow

1. **Register identity:**
   ```
   ensure_project(project_key=<abs-path>)
   register_agent(project_key, program, model)
   ```

2. **Reserve files before editing:**
   ```
   file_reservation_paths(project_key, agent_name, ["src/**"], ttl_seconds=3600, exclusive=true)
   ```

3. **Communicate with threads:**
   ```
   send_message(..., thread_id="FEAT-123")
   fetch_inbox(project_key, agent_name)
   acknowledge_message(project_key, agent_name, message_id)
   ```

4. **Quick reads:**
   ```
   resource://inbox/{Agent}?project=<abs-path>&limit=20
   resource://thread/{id}?project=<abs-path>&include_bodies=true
   ```

### Macros vs Granular Tools

- **Prefer macros for speed:** `macro_start_session`, `macro_prepare_thread`, `macro_file_reservation_cycle`, `macro_contact_handshake`
- **Use granular tools for control:** `register_agent`, `file_reservation_paths`, `send_message`, `fetch_inbox`, `acknowledge_message`

### Common Pitfalls

- `"from_agent not registered"`: Always `register_agent` in the correct `project_key` first
- `"FILE_RESERVATION_CONFLICT"`: Adjust patterns, wait for expiry, or use non-exclusive reservation
- **Auth errors:** If JWT+JWKS enabled, include bearer token with matching `kid`

---

## Beads (br) — Dependency-Aware Issue Tracking

Beads provides a lightweight, dependency-aware issue database and CLI (`br` - beads_rust) for selecting "ready work," setting priorities, and tracking status. It complements MCP Agent Mail's messaging and file reservations.

**Important:** `br` is non-invasive—it NEVER runs git commands automatically. You must manually commit changes after `br sync --flush-only`.

### Conventions

- **Single source of truth:** Beads for task status/priority/dependencies; Agent Mail for conversation and audit
- **Shared identifiers:** Use Beads issue ID (e.g., `br-123`) as Mail `thread_id` and prefix subjects with `[br-123]`
- **Reservations:** When starting a task, call `file_reservation_paths()` with the issue ID in `reason`

### Typical Agent Flow

1. **Pick ready work (Beads):**
   ```bash
   br ready --json  # Choose highest priority, no blockers
   ```

2. **Reserve edit surface (Mail):**
   ```
   file_reservation_paths(project_key, agent_name, ["src/**"], ttl_seconds=3600, exclusive=true, reason="br-123")
   ```

3. **Announce start (Mail):**
   ```
   send_message(..., thread_id="br-123", subject="[br-123] Start: <title>", ack_required=true)
   ```

4. **Work and update:** Reply in-thread with progress

5. **Complete and release:**
   ```bash
   br close 123 --reason "Completed"
   br sync --flush-only  # Export to JSONL (no git operations)
   ```
   ```
   release_file_reservations(project_key, agent_name, paths=["src/**"])
   ```
   Final Mail reply: `[br-123] Completed` with summary

### Mapping Cheat Sheet

| Concept | Value |
|---------|-------|
| Mail `thread_id` | `br-###` |
| Mail subject | `[br-###] ...` |
| File reservation `reason` | `br-###` |
| Commit messages | Include `br-###` for traceability |

---

## bv — Graph-Aware Triage Engine

bv is a graph-aware triage engine for Beads projects (`.beads/beads.jsonl`). It computes PageRank, betweenness, critical path, cycles, HITS, eigenvector, and k-core metrics deterministically.

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). For agent-to-agent coordination (messaging, work claiming, file reservations), use MCP Agent Mail.

**CRITICAL: Use ONLY `--robot-*` flags. Bare `bv` launches an interactive TUI that blocks your session.**

### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

```bash
bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command
```

### Command Reference

**Planning:**
| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with `unblocks` lists |
| `--robot-priority` | Priority misalignment detection with confidence |

**Graph Analysis:**
| Command | Returns |
|---------|---------|
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS, eigenvector, critical path, cycles, k-core, articulation points, slack |
| `--robot-label-health` | Per-label health: `health_level`, `velocity_score`, `staleness`, `blocked_count` |
| `--robot-label-flow` | Cross-label dependency: `flow_matrix`, `dependencies`, `bottleneck_labels` |
| `--robot-label-attention [--attention-limit=N]` | Attention-ranked labels |

**History & Change Tracking:**
| Command | Returns |
|---------|---------|
| `--robot-history` | Bead-to-commit correlations |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues, cycles |

**Other:**
| Command | Returns |
|---------|---------|
| `--robot-burndown <sprint>` | Sprint burndown, scope changes, at-risk items |
| `--robot-forecast <id\|all>` | ETA predictions with dependency-aware scheduling |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |
| `--export-graph <file.html>` | Interactive HTML visualization |

### Scoping & Filtering

```bash
bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank
bv --robot-triage --robot-triage-by-track    # Group by parallel work streams
bv --robot-triage --robot-triage-by-label    # Group by domain
```

### Understanding Robot Output

**All robot JSON includes:**
- `data_hash` — Fingerprint of source beads.jsonl
- `status` — Per-metric state: `computed|approx|timeout|skipped` + elapsed ms
- `as_of` / `as_of_commit` — Present when using `--as-of`

**Two-phase analysis:**
- **Phase 1 (instant):** degree, topo sort, density
- **Phase 2 (async, 500ms timeout):** PageRank, betweenness, HITS, eigenvector, cycles

### jq Quick Reference

```bash
bv --robot-triage | jq '.quick_ref'                        # At-a-glance summary
bv --robot-triage | jq '.recommendations[0]'               # Top recommendation
bv --robot-plan | jq '.plan.summary.highest_impact'        # Best unblock target
bv --robot-insights | jq '.status'                         # Check metric readiness
bv --robot-insights | jq '.Cycles'                         # Circular deps (must fix!)
```

---
## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

**Important:** `br` is non-invasive—it NEVER executes git commands. After `br sync --flush-only`, you must manually run `git add .beads/ && git commit`.

### Essential Commands

```bash
# View issues (launches TUI - avoid in automated sessions)
bv

# CLI commands for agents (use these instead)
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason "Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export to JSONL (NO git operations)
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Run `br sync --flush-only` then manually commit

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers, not words)
- **Types**: task, bug, feature, epic, question, docs
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads to JSONL
git add .beads/         # Stage beads changes
git commit -m "..."     # Commit everything together
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always `br sync --flush-only && git add .beads/` before ending session

<!-- end-bv-agent-instructions -->

---

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Sync beads** - `br sync --flush-only` to export to JSONL
5. **Hand off** - Provide context for next session

### Commit Discipline

- **Always run checks before committing**: For any code change, run tests, linters, builds,,format and `ubs $(git diff --name-only --cached)` on staged files before creating a commit.
- **Only commit when checks pass**: Do not commit if tests, linters, builds, or UBS are failing, unless you are explicitly committing a known-broken state with a clear reason in the commit message and associated issue.
- **Treat every change as commit-ready**: Work as if any local change could be committed; keep changes small, coherent, and fully validated before `git commit`.

---

Note for Codex/GPT-5.2:

You constantly bother me and stop working with concerned questions that look similar to this:

```
Unexpected changes (need guidance)

- Working tree still shows edits I did not make in Cargo.toml, Cargo.lock, src/cli/commands/upgrade.rs, src/storage/sqlite.rs, tests/conformance.rs, tests/storage_deps.rs. Please advise whether to keep/commit/revert these before any further work. I did not touch them.

Next steps (pick one)

1. Decide how to handle the unrelated modified files above so we can resume cleanly.
2. Triage beads_rust-orko (clippy/cargo warnings) and beads_rust-ydqr (rustfmt failures).
3. If you want a full suite run later, fix conformance/clippy blockers and re‑run cargo test --all.
```

NEVER EVER DO THAT AGAIN. The answer is literally ALWAYS the same: those are changes created by the potentially dozen of other agents working on the project at the same time. This is not only a common occurence, it happens multiple times PER MINUTE. The way to deal with it is simple: you NEVER, under ANY CIRCUMSTANCE, stash, revert, overwrite, or otherwise disturb in ANY way the work of other agents. Just treat those changes identically to changes that you yourself made. Just fool yourself into thinking YOU made the changes and simply don't recall it for some reason.

## Legacy `bd` Workflow (Deprecated)

Historical docs may still mention `beads_viewer`/`bd` commands. For this repository, that workflow is deprecated.

Canonical issue workflow is:
- `br` for task state and dependency management
- `br sync --flush-only` for JSONL export (no git automation)
- `bv --robot-*` for triage/planning (never bare `bv`)

Do not run `bd`/`bd sync` for normal work. Only use legacy command names when reading old artifacts or translating historical instructions.

Quick translation from legacy docs:

| Legacy | Canonical |
|--------|-----------|
| `bd ready` | `br ready` |
| `bd list --status=open` | `br list --status open` |
| `bd show <id>` | `br show <id>` |
| `bd update <id> --status=in_progress` | `br update <id> --status in_progress` |
| `bd close <id>` | `br close <id> --reason "Completed"` |
| `bd sync` | `br sync --flush-only` + manual git add/commit/push |

---

## UBS Quick Reference for AI Agents

UBS stands for "Ultimate Bug Scanner": **The AI Coding Agent's Secret Weapon: Flagging Likely Bugs for Fixing Early On**

**Install:** `curl -sSL https://raw.githubusercontent.com/Dicklesworthstone/ultimate_bug_scanner/master/install.sh | bash`

**Golden Rule:** `ubs <changed-files>` before every commit. Exit 0 = safe. Exit >0 = fix & re-run.

**Commands:**
```bash
ubs file.ts file2.py                    # Specific files (< 1s) — USE THIS
ubs $(git diff --name-only --cached)    # Staged files — before commit
ubs --only=js,python src/               # Language filter (3-5x faster)
ubs --ci --fail-on-warning .            # CI mode — before PR
ubs --help                              # Full command reference
ubs sessions --entries 1                # Tail the latest install session log
ubs .                                   # Whole project (ignores things like .venv and node_modules automatically)
```

**Output Format:**
```
⚠️  Category (N errors)
    file.ts:42:5 – Issue description
    💡 Suggested fix
Exit code: 1
```
Parse: `file:line:col` → location | 💡 → how to fix | Exit 0/1 → pass/fail

**Fix Workflow:**
1. Read finding → category + fix suggestion
2. Navigate `file:line:col` → view context
3. Verify real issue (not false positive)
4. Fix root cause (not symptom)
5. Re-run `ubs <file>` → exit 0
6. Commit

**Speed Critical:** Scope to changed files. `ubs src/file.ts` (< 1s) vs `ubs .` (30s). Never full scan for small edits.

**Bug Severity:**
- **Critical** (always fix): Null safety, XSS/injection, async/await, memory leaks
- **Important** (production): Type narrowing, division-by-zero, resource leaks
- **Contextual** (judgment): TODO/FIXME, console logs

**Anti-Patterns:**
- ❌ Ignore findings → ✅ Investigate each
- ❌ Full scan per edit → ✅ Scope to file
- ❌ Fix symptom (`if (x) { x.y }`) → ✅ Root cause (`x?.y`)

---

## cass — Cross-Agent Session Search

`cass` indexes prior agent conversations (Claude Code, Codex, Cursor, Gemini, ChatGPT, etc.) so we can reuse solved problems.

**Rules:** Never run bare `cass` (TUI). Always use `--robot` or `--json`.

### Examples

```bash
cass health
cass search "async runtime" --robot --limit 5
cass view /path/to/session.jsonl -n 42 --json
cass expand /path/to/session.jsonl -n 42 -C 3 --json
cass capabilities --json
cass robot-docs guide
```

### Tips

- Use `--fields minimal` for lean output
- Filter by agent with `--agent`
- Use `--days N` to limit to recent history

stdout is data-only, stderr is diagnostics; exit code 0 means success.

Treat cass as a way to avoid re-solving problems other agents already handled.

---

Note for Codex/GPT-5.2:

You constantly bother me and stop working with concerned questions that look similar to this:

```
Unexpected changes (need guidance)

- Working tree still shows edits I did not make in Cargo.toml, Cargo.lock, src/cli/commands/upgrade.rs, src/storage/sqlite.rs, tests/conformance.rs, tests/storage_deps.rs. Please advise whether to keep/commit/revert these before any further work. I did not touch them.

Next steps (pick one)

1. Decide how to handle the unrelated modified files above so we can resume cleanly.
2. Triage beads_rust-orko (clippy/cargo warnings) and beads_rust-ydqr (rustfmt failures).
3. If you want a full suite run later, fix conformance/clippy blockers and re-run cargo test --all.
```

NEVER EVER DO THAT AGAIN. The answer is literally ALWAYS the same: those are changes created by the potentially dozen of other agents working on the project at the same time. This is not only a common occurrence, it happens multiple times PER MINUTE. The way to deal with it is simple: you NEVER, under ANY CIRCUMSTANCE, stash, revert, overwrite, or otherwise disturb in ANY way the work of other agents. Just treat those changes identically to changes that you yourself made. Just fool yourself into thinking YOU made the changes and simply don't recall it for some reason.

---



## Note on Built-in TODO Functionality

Also, if I ask you to explicitly use your built-in TODO functionality, don't complain about this and say you need to use beads. You can use built-in TODOs if I tell you specifically to do so. Always comply with such orders.