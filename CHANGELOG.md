# Changelog

All notable changes to this project are documented here.

Format: grouped by release tag (newest first). `Thanks` credits non-maintainer contributors/authors/co-authors in that release range.

## [v0.9.22] - 2026-11-08
- fix(editor): block ops dropped expected_hash, accepting stale anchors (#126)
- fix(editor): block ops used 0-based span as 1-based, corrupting a page (#125)
- fix(patch): validate hash anchors for INS.PRE and INS.POST (#123)
- test: add CLI e2e suite (scripts/e2e.sh) covering all commands
- chore(release): add cargo-release config (tag-driven GitHub Release)
- docs: document the cargo-release flow (no crates.io, manual changelog)

## [v0.9.21] - 2026-11-07
- docs(mcp): make editing instructions and patch grammar self-contained (#122) — self-contained read→patch→verify workflow, patch grammar/payload escaping, JSON examples, expanded tool/param docs, schema & behavior tests. No parser/behavior/API changes.
- chore(release): bump to 0.9.21
- Thanks: @cleg (Paul Dmitryev) for #122.

## [v0.9.20] - 2026-10-21
- fix(patch): classify inserted vs modified lines, fail rejected patches

## [v0.9.19] - 2026-09-17
- fix(install): verified replaces, preserved MCP env, truthful self-update (#120)

## [v0.9.18] - 2026-09-16
- feat(patch): opt-in updated hash anchors in patch responses (#119)

## [v0.9.17] - 2026-09-15
- docs: redraw README architecture diagram (aadc-clean)
- Fix/install stale version warning (#116)
- install: delete stale binary copies instead of warning
- feat(errors): point unexpected failures at the issue tracker
- fix(release): sync Cargo.lock to 0.9.17 (--locked builds failed)
- fix(release): static release body instead of generate_release_notes (API 500s killed v0.9.17 publish)

## [v0.9.16] - 2026-09-06
- feat: add hashline update command + outdated-version notice (bd-pbn)
- chore: bump version to 0.9.16

## [v0.9.15] - 2026-09-05
- fix(#113,#114,#115): pipe separator, PUT format errors, stale anchor masking
- style: cargo fmt + clippy clean
- chore: .gitignore .omo/ and remove from tracking
- test: add E2E regression tests for #113, #114, #115
- chore: bump version to 0.9.15

## [v0.9.14] - 2026-08-28
- fix(installer): auto-replace existing binary on PATH at different location
- fix(installer): install.sh log_ok typo → log_success
- feat(integration): update pi-hashline for 0.9.12 compact output contract
- feat(integration): full hashline surface in pi — write, block ops, file tools, diff display
- docs(pi-hashline): two-step install guide — binary first, then npm package
- docs(readme): add pi-coding-agent guide to Agent Host Integrations
- feat(opencode-plugin): full hashline surface, compact contract, npm publish
- chore(beads): track scoped-npm followup
- fix(opencode-plugin): opencode2 compatibility — v2 Plugin.define entrypoint
- chore: bump to v0.1.9
- docs(opencode-plugin): split install guide for OpenCode 2.x (plugins array) vs 1.x (plugin)
- fix(pi-hashline): prevent unhandled EPIPE on child.stdin when subprocess exits early
- chore(pi-hashline): bump to 0.1.1 (EPIPE fix from #110)
- chore: bump 0.1.11 (latest for install)
- fix(v2): forward directory/worktree into tool context — path resolution was broken
- fix: ignore JavaScript template literal braces (#111)
- fix: reject patches with unknown op keywords atomically (#112)
- fix: prefix unused variable in test (clippy -D warnings)
- style: rustfmt fix
- chore: bump version to 0.9.14
- fix: update Cargo.lock to match version 0.9.14 (release --locked)
- Thanks: raphaelbahat, @christensenjairus, Jairus Christensen.

## [v0.9.13] - 2026-08-19
- feat: agent-first default output — compact text, --verbose for humans
- chore(beads): reflect — add 10 shipped beads for v0.8.4–v0.9.13

## [v0.9.12] - 2026-08-18
- feat(mcp): add instructions field to initialize response (fixes #107)
- style(mcp): run cargo fmt on test assertions
- chore: bump version to 0.9.12

## [v0.9.11] - 2026-08-17
- fix(patch): translate insert and block anchors through shift table (#106)
- fix(patch): improve multi-edit block ops and trailing blank shift tracking
- fix(patch): add anchor bounds check for INS.PRE/INS.POST operations
- style: fix clippy warning — collapse if into match guard
- chore: bump version to 0.9.11

## [v0.9.10] - 2026-08-11
- Low-breakage accuracy improvements (snapshot collision, noop guard, symbol seeding, recovery, boundary repair) (#105)
- docs(integration): refreshed detailed plan for pi-hashline + opencode-plugin from fresh .tmp research
- chore(integration): shared foundations — gitignore exception, golden fixtures, CLI CONTRACT
- feat(integration): implement pi-hashline + opencode-plugin thin-wrapper packages
- docs(README): add Agent Host Integrations table (pi-hashline, opencode-plugin)
- style(opencode-plugin): biome format 14 files (consistency with pi-hashline)
- fix(integration): replace placeholder paths with real repo locations
- chore: remove handoff file

## [v0.9.1] - 2026-08-06
- fix: DEL after bodied SWAP silently deleted wrong lines (fixes #104)
- chore: bump version to 0.9.1

## [v0.9.0] - 2026-07-30
- fix: A.=B range syntax broken (fixes #96)
- fix: line numbering mismatch, INS.POST 0 rejection, CRLF dry-run diff (fixes #102, #103, #101)
- chore: bump version to 0.9.0

## [v0.8.9] - 2026-07-29
- fix: SWAP.BLK/DEL.BLK range format silently corrupts files (fixes #95)
- chore: update Cargo.lock for v0.8.9
- chore: cargo fmt

## [v0.8.8] - 2026-07-28
- feat: Editor API with injectable SnapshotStore/BlockResolver for zero dot-folder embedding (Hướng 2) (#94)
- feat: add Editor::apply_to_text for in-memory patch (no disk I/O)
- feat: add Editor::apply_edits for pre-parsed edit application
- chore: remove redudant files

## [v0.8.7] - 2026-07-26
- Refactor code structure for improved readability and maintainability
- docs: refresh hero illustration — PNG→WEBP, robot workshop style
- docs: overhaul README to Dicklesworthstone-class style
- fix: preserve interior blank lines and bare `-` rows in patch/INS body (fixes #93)
- chore: bump version to 0.8.7 for release
- chore: update Cargo.lock for v0.8.7

## [v0.8.6] - 2026-07-09
- fix: accept bare 'INS <N>:' as shorthand for 'INS.POST <N>:'
- fix: replace byte-slicing with UTF-8-safe truncation to prevent panic on multi-byte lines

## [v0.8.5] - 2026-07-04
- fix: find-block off-by-one line_count for files with trailing newline (#91)
- chore: bump v0.8.5

## [v0.8.4] - 2026-07-01
- feat: remove replace tool, add REM (delete file) and MV (rename file) operations
- docs: fix backtick escaping in README HTTP API and agent integration sections
- fix: MV quoting, REM+edits validation, single-file-op guard per oh-my-pi compat
- cargo fmt: clean up formatting after compat fixes
- fix: clippy manual_ignore_case_cmp warning
- chore: bump v0.8.4
- chore: update Cargo.lock for v0.8.4
- Thanks: Claude.

## [v0.8.3] - 2026-06-30
- fix: remove stray YAML frontmatter delimiter from AGENTS.md
- Remove MCP str_replace-style replace tool (#88)
- Thanks: Claude, Dang Tran Quang.

## [v0.8.2] - 2026-06-23
- docs: remove why/scope from AGENTS.md, fix README format to show LINE:HASH|content
- docs: fix README table format LINE|content → LINE:hash|content
- Fix 4 open GitHub issues (#79, #85, #86, #87)
- fix: cargo fmt indentation in tokenizer.rs RangeScan
- test: 336-pass manual + edge test suite; fix: abort marker returns rc=0
- docs: guide agents to use stdin for multi-line patches (no .patch files)
- guide: add 'Patch Source Modes' section showing stdin as preferred
- chore: bump v0.8.2, fix clippy dead_code warning

## [v0.8.1] - 2026-06-21
- fix: issues #78, #80 — qualified anchor HashNotFound + relocation warning
- fix: cargo fmt, clippy needless_borrow — CI pass
- chore: bump v0.8.1, remove dead is_structural_closer_line

## [v0.8.0] - 2026-06-21
- chore: clean up stale worktree artifacts (.jcode, .ntm, release tarballs)
- feat: align with oh-my-pi hashline — all gap features
- fix: issues #66, #67, #68 + docs + clippy cleanup
- fix: issues #71, #72, #74 — DEL.BLK, Python find-block, generic errors
- chore: bump v0.8.0 — full oh-my-pi alignment + bug fixes

## [v0.7.8] - 2026-06-19
- feat: resolve all 12 open GitHub issues + v0.7.6 release
- fix: fast write is now default — no more temp files, no fsync
- fix: replace now never errors on ambiguity — warns and replaces first
- fix: hash suffix in SWAP/DEL/INS headers now validated against line content
- remove: replace command
- chore: bump to v0.7.8, remove replace command, clean up references
- Thanks: Claude.

## [v0.7.5] - 2026-06-18
- chore: update beads for v0.7.4 release
- test: comprehensive UTF-8 edge case tests for normalize_to_lf
- docs: document hashline guide command in README
- chore: bump to v0.7.5

## [v0.7.4] - 2026-06-18
- fix: normalize_to_lf corrupts multi-byte UTF-8 characters (#53)
- chore: bump to v0.7.4, fix Cargo.lock staleness

## [v0.7.3] - 2026-06-17
- fix: filter trailing empty line from read JSON output
- chore: bump to v0.7.3

## [v0.7.2] - 2026-06-17
- ci: update actions to latest versions for Node.js 24 support
- fix: show per-line hashes in read output, fix find-block hash display
- fix: cargo fmt
- chore: bump to v0.7.2

## [v0.7.1] - 2026-06-17
- feat: add hashline guide command
- docs: add Agent Integration section to README
- fix: HASHLINE_URL silent output with structuredContent.content[].text

## [v0.7.0] - 2026-06-16
- chore: bump to v0.7.0, fix CI checks
- chore: update Cargo.lock for v0.7.0

## [v0.6.0] - 2026-06-16
- feat: complete hashline migration to oh-my-pi compatible architecture
- fix: address clippy warnings and format issues
- fix: INS.TAIL trailing newline issue and normalize split/join everywhere
- fix: empty file handling in join_lines and MCP path
- docs: update README with comparison table, ASCII workflow, and current benchmarks
- docs: simplify comparison table to hashline vs str_replace only
- docs: simplify benchmark tables (remove throughput column)
- docs: benchmark table to 3-column feature-hashline-str_replace
- docs: fill benchmark table with hashline vs str_replace measurements
- chore: fix formatting, bump version to 0.6.0 for release
- cleanup: remove all oh-my-pi references; fill micro benchmark str_replace column
- fix: resolve clippy nonminimal_bool lint in prefixes.rs
- fix: apply cargo fmt to satisfy CI formatting check
- fix: restore handle_index, handle_annotate, handle_grep functions removed by rustfmt bug
- mcp: reduce tools to match CLI commands (read, patch, find_block only)
- fix: remove extra blank lines to satisfy CI rustfmt check
- Thanks: Claude.

## [v0.5.0] - 2026-06-14
- docs: simplify benchmark table in README
- docs: rewrite benchmarks in English with ASCII flow diagrams
- feat: add stale anchor recovery and repair heuristics inspired by oh-my-pi
- fix: cargo fmt after ultracode workflow changes
- chore: bump version to v0.5.0
- refactor: remove broken hashline_watch, hashline_watch_capabilities, hashline_merge_patches
- Thanks: Claude.

## [v0.4.0] - 2026-06-14
- test(perf): #37 verify after_mutation seeds cache across sequential edits
- perf: #38 wire load_with_hash_cache into all mutation commands
- perf: #41 add xxh3 64-bit hash alongside xxh32
- feat: #43 stale-anchor auto-refresh — invalidate cache and retry once
- feat(api): #45 sticky Document in session cache
- feat(api): #40 streaming edit for large files
- feat(api): #44 range/region edit by content query
- feat(api): #46 diff-first workflow (hashline_apply_diff)
- feat(api): #39 batch mutation API
- feat(api): #42 compact output mode for hashline_read
- Fix PR #48 review issues + add fast paths to reach str_replace speed
- Add fast_edit benchmark to edit_comparison
- Add fast paths for range edits and content-query edits
- Add fast paths for indent range and patch single-edit
- Add indent range fast path for negative amounts (dedent)
- Add multi-op patch fast path and indent dedent fast path
- Cleanup: fix all clippy warnings and unused imports
- Add fast_from_hash and fast_fuzzy_resolve to fast_edit.rs
- run_fast_edit: add dry_run, guard mtime/inode support
- run_fast_insert: add dry_run, guard mtime/inode support
- Complete fast_* migration: all commands support guards, dry-run, raw hash, fuzzy, mixed indent
- Remove restrictive conditions from fast paths - all commands use fast_* as default
- Move fast_edit.rs to crate::fast module
- Remove dry_run/guard conditions from fast path - handled by run_fast_edit internally
- feat(fast): add interpret_escapes, receipt, audit_log to fast path
- fix review findings: remove fast path guards, fix receipt ordering, complete move receipt, align interpret_escapes
- fix: gate unix-specific MetadataExt behind #[cfg(unix)] for Windows build
- refactor: merge fast path into unified hashline with Document pipeline fallback
- feat: all operations use fast path as default, remove feature guards
- chore: bump version to v0.4.0 for fast path migration
- style: cargo fmt --all to fix CI formatting check
- fix: cargo fmt after merge resolution
- Thanks: Claude.

## [v0.3.11] - 2026-06-11
- fix: harden release.yml for Windows CI resilience (#36)

## [v0.3.10] - 2026-06-11
- fix: hashline_symbol scope returning empty results; find-block failing on Verse files (#36)
- chore: bump version to 0.3.10
- Thanks: Claude.

## [v0.3.9] - 2026-06-11
- fix: handle in-use binary on Windows upgrade (#34)
- Thanks: Claude.

## [v0.3.8] - 2026-06-11
- fix: handle in-use binary on Windows upgrade (#34)
- Thanks: Claude.

## [v0.3.7] - 2026-06-11
- fix: update line-shift benchmark to respect fuzzy relocation radius
- Thanks: Claude.

## [v0.3.6] - 2026-06-10
- fix: upgrade path on Windows (#33) + missing MCP dispatch (#32)

## [v0.3.5] - 2026-06-10
- chore: bump to v0.3.5 (include Cargo.lock)
- chore: bump to v0.3.5 (with Cargo.lock)

## [v0.3.4] - 2026-06-10
- fix: robust error handling and diagnostics in install.ps1
- chore: bump to v0.3.4

## [v0.3.3] - 2026-06-10
- fix: Windows .sha256 upload - use Out-File and single glob pattern
- chore: bump to v0.3.3

## [v0.3.2] - 2026-06-10
- fix: use unambiguous ${var}.suffix syntax in PowerShell scripts
- fix: resolve version scoping in install.ps1, add Intel Mac to release matrix
- chore: bump to v0.3.2

## [v0.3.1] - 2026-06-10
- Maintenance release.

## [v0.3.0] - 2026-06-10
- feat: implement hashline_grep CLI + MCP tool
- feat: implement hashline_annotate CLI + MCP tool
- chore: remove stray sidecar temp files from commit
- feat: implement hashline_explode, hashline_implode, hashline_watch MCP tools
- feat: implement hashline_find_block CLI + MCP tool
- feat: implement remaining Phase 1 MCP tools (map, symbol, callees, from_diff, merge_patches)
- feat(phase2): extract SessionState into public session_cache module with LRU + stats
- fix: remove stray sidecar temp files
- feat(phases3-5): daemon mode + HASHLINE_SOCKET routing + MCP proxy
- fix CI/CD: resolve clippy lints, formatting drift, and rustc nightly breakage
- fix: cfg(unix) guards for Windows cross-compile CI
- fix: suppress unused lock_file warning on Windows (clippy -D warnings)
- fix: cfg(unix) guard get_socket_env (dead on Windows clippy -D warnings)
- chore: bump to v0.3.1, update README tool counts
- Thanks: Claude Opus 4.8.

## [v0.2.2] - 2026-06-07
- remove .hashline/skills workflow pack logic (unused)
- remove broken submodule benchmark/fixtures/repo (no URL configured)
- delete LICENSE-APACHE (MIT only)
- fix: cargo fmt --all to fix CI formatting issue in error.rs
- chore: bump version to 0.2.2
- fix: regenerate Cargo.lock after version bump (fixes --locked CI failures)

## [v0.2.1] - 2026-05-24
- feat: AI-friendly output, fuzzy relocation, port install-mcp to shell
- feat(cli): trim 17 commands; focus on read/edit core (ffs handles search)
- feat: rename linehash → hashline (binary, crate, MCP, configs, docs)
- chore: remove dead code (SearchDocCache), drop unused deps
- bench: rewrite bench-features.sh for 13 current commands; add fresh results
- fix: clippy + rustfmt (CI)
- chore: delete stale plan docs; refresh benchmark results
- chore: publishable metadata + LICENSE files (v0.1.11)
- feat: split into library + binary (v0.2.0)
- fix(orchestration): move run_command block above test mod
- feat: SHA-256 backward-compat anchors behind feature flag (v0.2.1)
- bench: add feature benchmark results for v0.2.1
- bench: add hashline vs sed (str-replace) comparison
- bench: integrate sed/str-replace comparison into bench script
- fix(install.ps1): strip UTF-8 BOM so `irm | iex` parses on Windows PowerShell 5.1
- fix(install.ps1): unblock Windows install end-to-end (TLS 1.2, $home, ASCII)

## [v0.1.10] - 2026-05-23
- Add full-feature benchmark table and reproducible bench script
- bench: upgrade benchmark suite with throughput, scaling, and search_bench
- perf: zero-copy edit, streaming grep, faster trigram lookups
- perf: streaming grep --json/--ndjson, FxHashMap for trigram index
- perf(grep): batch ndjson writes via per-line scratch buffer
- perf(load): wire up Document::load_with_hash_cache in read-only commands
- perf(document): swap LineRecord.content from Arc<str> to Box<str>
- perf(output): fast-path JSON string serializer via memchr3
- fmt: rustfmt fixture vec literal
- feat(install): add Windows install.ps1 for `irm | iex` install
- ci(release): fix macOS runner labels for GitHub Actions (#18)
- chore: bump version to v0.1.10
- chore: update Cargo.lock after version bump
- fix(release): set fail-fast true to cancel all jobs when any fails
- fix(release): remove linux-aarch64 from build matrix
- fix(release): restore fail-fast: false to allow all builds to complete
- fix(release): add 'if: always()' to release job so it runs even when builds fail
- fix(release): add concurrency control and remove if:always() to fix stuck workflow runs
- fix(release): add workflow_dispatch for manual trigger
- fix(release): remove macos-x86_64 from build matrix due to stuck runner
- Thanks: Devin AI, Claude Opus 4.7.

## [v0.1.9] - 2026-05-17
- Fix release/UX issues found during v0.1.8 smoke test

## [v0.1.8] - 2026-05-16
- feat: support multi-line content in linehash_insert
- fix: use sort_by_key instead of sort_by for clippy
- refactor: remove obsolete optimization plans and project metadata files
- chore: remove obsolete skill documentation and file write hook
- feat: add benchmark framework for linehash performance evaluation
- feat: enhance benchmark framework with repository configuration and task definitions
- fix: unblock build by importing index helpers and accepting &Document in resolve_read_anchors
- chore(clippy): clean up workspace warnings to gate CI with -D warnings
- perf(stats): compute collision_pair_count in closed form, cap pairs sample
- feat(outline): guard input size before invoking tree-sitter
- Resolve outline merge conflicts
- feat(output): compact JSON default, --pretty opt-in, dry-run receipts, NDJSON
- fix(atomic_write): handle bare relative paths without parent dir
- perf(lang): thread-local tree-sitter Parser cache + consolidate language map
- perf: parallel line hashing + buffered stdout/stderr
- perf(read): zero-clone streaming JSON / NDJSON output
- perf: split parse_document_content into sequential/parallel paths
- bench: add 2026-05-16 local feature + benchmark report
- Add latency roadmap beads
- feat: latency roadmap Tier 1 & Tier 2 implementation with validation
- Close all 7 open beads: zero-copy LineRecord, SIMD grep, hash sidecar cache
- Fix compiler warnings: remove unused content_len, allow dead_code on invalidate
- Fix CI: add hash_cache mod to benches, fix clippy useless_conversion, apply rustfmt
- Fix Windows CI: add retry loop for file persist (ACCESS_DENIED race)
- Fix Windows CI: exponential backoff + direct-write fallback for persist race
- Fix Windows CI: use SeekFrom instead of rewind for Windows compat
- Fix Windows CI: import Seek trait for Windows persist fallback
- Fix Windows CI: suppress unused variable in fallback match arm
- Fix Windows CI: drop mmap before atomic_write to prevent error 1224
- Thanks: Claude Opus 4.6, @Copilot.

## [v0.1.7] - 2026-04-12
- chore(readme): adjust section README
- chore(benchmark): add new benchmark
- polish: add explicit AC, test obligations, and failure modes to all instant-grep beads
- fix: Add T9/T10→T11 explicit blockers, add edge case comments to search beads
- docs: add instant-grep plan and polished beads (21 tasks)
- feat(search): T7 - IndexStore persistence layer
- feat(search): T8 - Auto-index on read
- T9: Regex to Trigram Decomposition
- T10: Candidate Filtering Using Masks
- T11: Full Regex Verification on Candidates
- T12: CLI grep Auto-Uses Index + --no-index Flag
- T13: MCP tool_grep Uses Index Automatically
- T14-T17: Incremental cache, stats, benchmarks, docs for trigram search
- Close trigram search beads T9-T17
- Close remaining T-beads
- Close F-beads: plan compliance, code quality, scope fidelity all verified
- cleanup: fix _Dot variable name, remove verify_line (unused), add #[allow(unused)] for API re-exports
- Optimize render() loop: eliminate index check per iteration
- edit: pre-build index before single-line resolve to avoid double-build
- perf: benchmark refresh - 100k mutate_render improved 50%, edit 55% gap closed vs naive
- style: run cargo fmt for CI consistency
- style: add allow(unused) to suppress pre-existing clippy warnings in search module
- fix clippy: add allow(unused) to search module, fix redundant closure, rename _Dot
- revert: use linear grep by default
- perf: optimize linear grep with fast path for simple patterns
- perf: add SearchDocument fast-path for grep (skip hash computation)
- feat: add server mode for persistent document cache
- Integrate auto-start daemon into MCP server
- Fix ensure_daemon_running to use shell for proper backgrounding
- Fix dead_code warnings in server.rs
- style: apply rustfmt to all files
- fix: use std::io::Error::other instead of Error::new
- fix: make server module Unix-only and guard cfg usage
- fix: unblock cross-platform clippy and bench builds
- fix: remove windows-only dead import in grep command
- fix: strip CR from grep JSON output on CRLF files

## [v0.1.6] - 2026-04-01
- Downgrade recoverable linehash rejections in logs
- linehash-5s0 tighten MCP read workflow and mutation guidance
- Fix dashed range hint routing
- linehash: share CLI and MCP orchestration for linehash-261.3
- linehash-261.2 Centralize destructive risk assessment
- linehash-128 Close implemented MCP cache bead
- linehash-261.1 Package markdown workflow packs
- linehash-261 Close Claude Code integration epic
- linehash-3qi Codify MCP watch streaming limits
- linehash-2lk Reduce test-only UBS warning inventory
- ci: stabilize release target runners
- fix: resolve clippy regressions
- fix: normalize workflow source paths
- fix: harden macOS install and release

## [v0.1.5] - 2026-03-30
- Add tracing and harden find-block brace scan
- Log installed linehash runs to a file
- Harden MCP install path and cover ranged edits
- fix: gate unix-only test imports for windows clippy
- fix: normalize symlink test path on macos

## [v0.1.4] - 2026-03-29
- Support ranged delete and multiline range edits
- Handle timestamped unified diff headers

## [v0.1.3] - 2026-03-29
- feat: auto-install MCP providers from installer
- fix: satisfy clippy in install host loop
- Thanks: Claude Opus 4.6.

## [v0.1.2] - 2026-03-29
- fix: use GitHub release version in install summary instead of binary version
- feat: optimize linehash IO and add auto-installing MCP server
- Thanks: Claude Opus 4.6.

## [v0.1.1] - 2026-03-26
- chore(docs): init docs
- docs(agent): added `AGENT.md`
- docs(agent): update `AGENT.md` with additional details
- docs(agent): enhance `AGENT.md` with new sections and clarifications
- docs(PLAN): add comprehensive module implementation specifications and detailed algorithms for line hashing and document loading
- docs(agent): introduce commit discipline guidelines in `AGENTS.md`
- feat: add .gitignore for local config and caches; create initial issues.jsonl with linehash v1 implementation tasks
- chore: remove obsolete implementation plans for linehash from `plan-claude.md` and `plan-codex.md`
- feat: bootstrap linehash workspace and command framework
- chore: add core crate manifest
- feat: add linehash foundation modules
- feat: add test harness and fixtures
- feat: add anchor parsing and resolution
- feat: add shared read and index formatters
- feat: implement read-side CLI commands
- feat: add shared mutation primitives
- feat: implement edit command
- feat: implement insert/delete commands and strengthen error hints
- test: validate optimistic concurrency guards
- feat: implement patch command
- chore: remove commit discipline section from AGENTS.md
- fix: harden atomic writeback safety
- feat: implement stats command
- feat: add mutation receipts and audit logging
- feat: implement from-diff compiler
- feat: implement indent command
- feat: implement find-block command
- feat: implement explode workflow support
- feat: implement watch command and validate README UX
- feat: add move command and hash benchmark target
- feat: implement swap and improve stale-anchor diagnostics
- feat: implement implode command for reconstructing exploded files
- feat: add snapshot coverage for stable CLI output
- fix: update watch test expectation
- chore: format core modules and sync bead state
- chore: close release hardening epic
- test: add acceptance coverage for swap and newline preservation
- test: cover stale and ambiguous edit anchors
- test: format ambiguous edit anchor assertion
- docs: clarify hash computation details in README
- docs: validate README workflows and fix patch dry-run wording
- fix: format patch dry-run message
- test: snapshot stale and ambiguous edit errors
- docs: improve README SEO positioning
- docs: restore and document node poc
- test: add comprehensive performance benchmarks
- perf: optimize core operations and refresh benchmarks
- refactor: enhance performance metrics and update benchmark results
- test: add edit benchmark matrix and reports
- test: enhance benchmark scenarios and reporting
- refactor: streamline document line management and rendering
- refactor: enhance receipt handling in document commands
- refactor: simplify line management and update benchmarks
- add release workflows and installer
- fix: resolve CI lint failures
- test: update newline detection in JSON parsing test
- test: format newline fixture assertion
- test: stabilize watch and snapshot CI checks
- fix: update file watching to use the parent directory
- refactor: update file watcher to use PollWatcher
- Revert "refactor: update file watcher to use PollWatcher"
- test: ignore flaky tests on macOS for file watch functionality
- docs: add linehash editing guidelines to AGENTS.md
- perf: add incremental short-hash index maintenance
- Revert "perf: add incremental short-hash index maintenance"
- docs(AGENTS.md): rewrite linehash section with comprehensive documentation
- docs(AGENTS.md): add why CLI documentation and commit discipline
- chore: update AGENTS.md to use scope
- feat: add workflow guidance and benchmark reporting
- perf: isolate 100k benchmark hotspots
- chore: cargo fmt and update benchmark results
- fix: resolve clippy warnings (if_same_then_else, iter_cloned_collect)
- Thanks: Claude Opus 4.6.

## Thanks

Thanks to everyone who contributed across releases: @quangdang46, Claude Opus 4.6, @Copilot, Devin AI, Claude Opus 4.7, Claude Opus 4.8, Claude, Dang Tran Quang, raphaelbahat, @christensenjairus, Jairus Christensen, Paul Dmitryev.
