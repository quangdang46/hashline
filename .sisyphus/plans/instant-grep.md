# linehash Instant Grep: Cursor-Style Trigram Index + Auto-Index + MCP

## TL;DR

> **Quick Summary**: Auto-indexing trigram grep for linehash — index builds on first `read`, grep uses index automatically, MCP integration for primary interface.
>
> **Key Change from Previous Plan**: NO `--indexed` flag. Index is ALWAYS used when available. `grep` command auto-detects and uses index. CLI option `--no-index` to force linear scan. MCP tools get first-class index support.
>
> **Deliverables**:
> - New `crates/core/search/` module with trigram index data structures
> - Auto-index on `read` (background build)
> - `linehash grep` auto-uses index, fallback to linear if not available
> - `--no-index` flag to force linear scan (opt-out)
> - MCP server: `tool_grep` uses index automatically
> - Incremental index updates via mtime/content_hash validation
> - Backward-compatible: existing grep output identical

---

## Context

### Key Behavior Change (from user feedback)
User confirmed: **auto-index should be the default, no flag needed**
- `linehash read <file>` → trigger background index build
- `linehash grep <file> <pattern>` → ALWAYS use index if valid
- `--no-index` flag for when you need to opt-out
- MCP is the primary interface → MCP tools must support auto-indexing

### User Requirements
1. Auto-index on read (background)
2. Grep uses index automatically (no flag)
3. MCP integration (primary interface for linehash)
4. Index stored persistently in `.linehash/indexes/`
5. Stale detection via mtime + content_hash
6. Fallback to linear if index unavailable/invalid

### Research Findings (unchanged)
1. **Cursor Blog**: Trigram inverted index → 13ms vs 16.8s ripgrep
2. **Complexity**: O(n) scan vs O(trigram_lookup + candidate_verify) → 20-100× speedup

---

## New Behavior Specification

### Auto-Index Flow
```
linehash read <file>
  → Load document
  → [Background] Build trigram index → persist .linehash/indexes/<rel>.lhidx

linehash grep <file> <pattern>
  → Check index cache (valid? mtime + content_hash match?)
  → If valid → USE INDEX (fast path)
  → If stale/missing → Build index inline → USE INDEX
  → Return results

linehash grep <file> <pattern> --no-index
  → Force linear O(n) scan
  → Ignore existing index
```

### MCP Integration (PRIMARY)
```rust
// mcp.rs - tool_grep() must use index automatically
fn tool_grep(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
    let cmd: GrepCmd = parse_args(arguments)?;
    let entry = session.get(&cmd.file)?;  // Uses session cache
    
    // NEW: Check for valid trigram index
    let search_result = if let Some(trigram_index) = session.get_trigram_index(&cmd.file)? {
        indexed_grep(&trigram_index, &entry.doc, &cmd.pattern, cmd.case_insensitive)?
    } else {
        // Fallback to linear grep
        grep_lines(&entry.doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
    };
    
    // Return results
}
```

### Index Validity
```
Index valid IF:
  ✓ File mtime == index.mtime
  ✓ File content_hash == index.content_hash
  ✗ Either fails → STALE → rebuild
```

---

## Work Objectives

### Core Objective
Implement Cursor-style instant grep with AUTO-INDEXING for linehash, with full MCP integration.

### Concrete Deliverables
- [ ] `crates/core/search/` module with trigram index types and operations
- [ ] Auto-index building triggered on `read` (background)
- [ ] `linehash grep` auto-uses index (default behavior)
- [ ] `--no-index` flag to force linear scan
- [ ] MCP `tool_grep` uses index automatically
- [ ] Persistent index storage under `.linehash/indexes/`
- [ ] Incremental index updates via file metadata validation
- [ ] Performance benchmarks comparing indexed vs linear grep

### Definition of Done
- [ ] `linehash read test.rs` builds index in background
- [ ] `linehash grep test.rs "pattern"` returns same results as before (but faster)
- [ ] Indexed grep on 10k+ line file completes in <50ms vs 500ms+ linear
- [ ] Index rebuilds automatically when file changes
- [ ] MCP `tool_grep` uses trigram index automatically
- [ ] All existing tests pass (backward compatibility)

### Must Have
- Auto-indexing (index builds without explicit command)
- Single-file trigram index (not multi-file search)
- Persistent binary index format
- Correctness: indexed grep produces identical results to linear grep
- MCP integration (primary interface)
- Incremental rebuild on file change

### Must NOT Have
- No `--indexed` flag (auto-index is default)
- No multi-file/repo-wide search (out of scope for initial version)
- No LSP/semantic understanding
- No network or cloud features
- No changes to existing anchor resolution behavior

---

## Execution Strategy

### Parallel Execution Waves

```
Wave 1 (Foundation — types and storage):
├── T1: Define search types (TrigramIndex, Posting, Mask)
├── T2: Design binary index format and serialization
├── T3: Implement index file read/write (mmap)
├── T4: Add search module to Cargo.toml

Wave 2 (Core indexing engine):
├── T5: Trigram extraction from line content
├── T6: Inverted index construction
├── T7: Index persistence to .linehash/indexes/
├── T8: CLI: auto-index on read (background build)

Wave 3 (Query engine and integration):
├── T9: Regex to trigram decomposition
├── T10: Candidate filtering using masks
├── T11: Full regex verification on candidates
├── T12: CLI: grep auto-uses index + --no-index flag
├── T13: MCP: tool_grep uses index automatically

Wave 4 (Polish and optimization):
├── T14: Incremental index updates (mtime/size tracking)
├── T15: Index stats and diagnostics
├── T16: Performance benchmarks (indexed vs linear)
├── T17: Documentation and examples

Wave FINAL (Verification — 4 parallel reviews):
├── F1: Plan compliance audit (oracle)
├── F2: Code quality review
├── F3: Real manual QA (grep verification)
├── F4: Scope fidelity check
```

### Dependency Matrix

- **T1-T4**: None — Wave 1 foundation
- **T5-T6**: T1 (types must exist first)
- **T7**: T2 (serialization needs format), T6 (index data)
- **T8**: T3 (file I/O), T4 (module exists), T6 (index building)
- **T9-T11**: T1 (types), T5-T6 (index building blocks)
- **T12**: T8 (read auto-index exists), T11 (query engine complete)
- **T13**: T12 (grep auto-index exists), T11 (query engine)
- **T14**: T7 (persistence), T13 (integration point)
- **T15**: T14 (incremental builds need tracking)
- **T16**: T12 (need working indexed grep to benchmark)
- **T17**: T16 (benchmarks complete)
- **F1-F4**: T17 (all implementation complete)

---

## TODOs

> Every task has: Recommended Agent Profile + Parallelization info + QA Scenarios.

- [ ] 1. Define Trigram Index Types

  **What to do**:
  - Create `crates/core/search/types.rs` with core data structures:
    ```rust
    /// 3-byte trigram as 24-bit integer (for efficient hashing/indexing)
    pub type Trigram = u32;
    
    /// Position mask: 8-bit mask, bit i set if trigram starts at pos % 8 == i
    #[derive(Clone, Copy, Default)]
    pub struct LocMask(pub u8);
    
    /// Next-char mask: 8-bit bloom filter of characters following trigram
    #[derive(Clone, Copy, Default)]
    pub struct NextMask(pub u8);
    
    /// Single posting: (line_index, loc_mask, next_mask)
    #[derive(Clone, Debug)]
    pub struct Posting {
        pub line_idx: usize,
        pub loc_mask: LocMask,
        pub next_mask: NextMask,
    }
    
    /// Trigram index: maps trigram → list of postings
    pub struct TrigramIndex {
        pub trigrams: FxHashMap<Trigram, Vec<Posting>>,
        pub line_count: usize,
    }
    
    /// Persisted index metadata (for validity check)
    pub struct IndexMeta {
        pub file_mtime: u64,
        pub file_size: u64,
        pub content_hash: u64,  // xxHash of full content for change detection
    }
    ```
  - Use `fxhash` crate (already available in workspace) for HashMap
  - Keep masks as `u8` wrapping types for type safety
  - Document all public types

  **Must NOT do**:
  - No actual index building logic yet
  - No file I/O yet
  - No query engine

  **Acceptance Criteria**:
  - [ ] Types compile: `Trigram`, `LocMask`, `NextMask`, `Posting`, `TrigramIndex`, `IndexMeta`
  - [ ] `cargo check --package linehash-core` succeeds

---

- [ ] 2. Design Binary Index Format

  **What to do**:
  - Design `crates/core/search/format.md`:
    ```
    Header (24 bytes):
      - magic: [0xC7, 0x7R, 0xG, 0xP] (4 bytes)
      - version: u32 LE (4 bytes)
      - flags: u32 LE (4 bytes)
      - line_count: u32 LE (4 bytes)
      - trigram_count: u32 LE (4 bytes)
      - meta_offset: u64 LE (8 bytes) — offset to metadata section
    
    Lookup Table (trigram_count * 12 bytes):
      - For each trigram (sorted ascending):
        - trigram_key: u32 LE (4 bytes)
        - posting_offset: u64 LE (8 bytes)
    
    Postings Section (variable):
      - For each trigram:
        - posting_count: u32 LE (4 bytes)
        - postings: [posting_count * 6 bytes]
          - line_idx: u32 LE (4 bytes)
          - loc_mask: u8 (1 byte)
          - next_mask: u8 (1 byte)
          - padding: u16 (2 bytes)
    
    Metadata Section (at meta_offset):
      - file_mtime: u64 LE (8 bytes)
      - file_size: u64 LE (8 bytes)
      - content_hash: u64 LE (8 bytes)
      - magic_end: u64 LE (0xDEADBEEFCAFEBABE)
    ```
  - Include rationale for design decisions
  - Document version handling for future compatibility

---

- [ ] 3. Implement Index Serialization

  **What to do**:
  - Create `crates/core/search/persist.rs`:
    ```rust
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, Seek};
    use std::path::Path;
    
    const MAGIC: [u8; 4] = [0xC7, 0x7R, 0xG, 0xP];
    const VERSION: u32 = 1;
    const MAGIC_END: u64 = 0xDEADBEEFCAFEBABE;
    
    impl TrigramIndex {
        /// Write index to file in binary format
        pub fn write_to(&self, path: &Path, meta: &IndexMeta) -> Result<(), SearchError>;
        
        /// Read index from file (mmap'd for efficient access)
        pub fn read_from(path: &Path) -> Result<(MmapIndex<'_>, IndexMeta), SearchError>;
        
        /// Check if index is valid for given file metadata
        pub fn is_valid(&self, file_mtime: u64, file_size: u64, content_hash: u64) -> bool;
    }
    
    /// Memory-mapped index for fast readonly access
    pub struct MmapIndex<'a> {
        data: &'a [u8],
        lookup: Vec<(Trigram, u64)>,
        meta: IndexMeta,
    }
    ```
  - Use `memmap2` crate for memory mapping
  - Implement validity check via metadata comparison

---

- [ ] 4. Add Search Module to Cargo.toml

  **What to do**:
  - Edit `crates/core/Cargo.toml` to add search module
  - Create `crates/core/search/mod.rs` with module declarations:
    ```rust
    pub mod types;
    pub mod persist;
    pub mod extract;   // T5: trigram extraction
    pub mod index;     // T6: index construction
    pub mod query;     // T9-T11: query engine
    ```
  - Ensure `lib.rs` exports search module

---

- [ ] 5. Implement Trigram Extraction

  **What to do**:
  - Create `crates/core/search/extract.rs`:
    ```rust
    /// Extract all overlapping trigrams from a string
    /// "hello" → ["hel", "ell", "llo"]
    pub fn extract_trigrams(content: &str) -> Vec<(Trigram, LocMask, NextMask)>;
    
    /// Convert 3 bytes to 24-bit trigram key
    fn trigram_from_bytes(bytes: &[u8]) -> Trigram;
    
    /// Hash a byte into 8-bit mask using low bits of CRC-like function
    fn hash_byte(b: u8) -> u8;
    ```
  - Handle edge cases: strings < 3 chars return empty
  - Use `LocMask` and `NextMask` types from T1

---

- [ ] 6. Build Inverted Index from Trigrams

  **What to do**:
  - Create `crates/core/search/index.rs`:
    ```rust
    impl TrigramIndex {
        /// Build index from document lines
        pub fn from_lines(lines: &[String]) -> Self;
        
        /// Insert a posting into the index
        fn insert(&mut self, trigram: Trigram, line_idx: usize, loc: LocMask, next: NextMask);
        
        /// Sort trigrams, consolidate duplicate line_idx entries
        fn freeze(&mut self);
    }
    ```
  - Handle duplicate trigrams in same line (OR masks)
  - Return sorted index ready for serialization

---

- [ ] 7. Persist Index to .linehash/indexes/

  **What to do**:
  - Extend `crates/core/search/persist.rs`:
    ```rust
    impl TrigramIndex {
        /// Save index to standard location: .linehash/indexes/{relative_path}.lhidx
        pub fn save(&self, file_path: &Path) -> Result<PathBuf, SearchError>;
        
        /// Load index from standard location
        pub fn load(file_path: &Path) -> Result<Option<(MmapIndex, IndexMeta)>, SearchError>;
        
        /// Load index if valid, rebuild if stale
        pub fn load_or_rebuild(file_path: &Path) -> Result<(MmapIndex, IndexMeta), SearchError>;
    }
    ```
  - Create `.linehash/indexes/` directory if not exists
  - Store metadata (mtime, size, content_hash) in index file

---

- [ ] 8. Auto-Index on Read (Background Build)

  **What to do**:
  - Modify `crates/core/commands/read.rs`:
    ```rust
    pub fn run<W: Write, E: Write>(
        ctx: &mut CommandContext<'_, W, E>,
        cmd: ReadCmd,
    ) -> Result<(), LinehashError> {
        let doc = Document::load(&cmd.file)?;
        
        // NEW: Trigger background index build
        spawn_index_build_background(&cmd.file, &doc);
        
        // Continue with normal read...
        let payload = read_payload(&doc, &cmd.anchors, cmd.context)?;
        // ... output ...
    }
    
    fn spawn_index_build_background(file_path: &Path, doc: &Document) {
        // Check if index exists and is valid
        // If not, spawn background task to build index
        // Use std::thread or tokio for async background build
    }
    ```
  - Index builds in background without blocking the read operation
  - If index already exists and is valid, skip rebuilding

---

- [ ] 9. Regex to Trigram Decomposition

  **What to do**:
  - Create `crates/core/search/query.rs`:
    ```rust
    pub enum TrigramQuery {
        All(Vec<Trigram>),
        Any(Vec<TrigramQuery>),
        None,
    }
    
    pub fn decompose_regex(pattern: &str) -> Result<TrigramQuery, InvalidPattern>;
    ```
  - Handle common regex patterns: literals, character classes, quantifiers
  - Return `TrigramQuery::None` for patterns that can't be indexed (e.g., `.*`)

---

- [ ] 10. Candidate Filtering Using Masks

  **What to do**:
  - Extend `crates/core/search/query.rs`:
    ```rust
    impl TrigramQuery {
        pub fn find_candidates(
            &self,
            index: &MmapIndex,
        ) -> Result<CandidateSet, SearchError>;
    }
    
    #[derive(Default)]
    pub struct CandidateSet {
        lines: Vec<usize>,
        all: bool,
    }
    ```
  - Implement efficient set operations (sorted vectors)
  - Handle `all: true` case for `None` query

---

- [ ] 11. Full Regex Verification on Candidates

  **What to do**:
  - Extend `crates/core/search/query.rs`:
    ```rust
    pub fn indexed_grep(
        index: &MmapIndex,
        lines: &[String],
        pattern: &str,
        case_insensitive: bool,
    ) -> Result<Vec<usize>, SearchError>;
    ```
  - Decompose regex → find candidates → verify with full regex
  - Return line indices of matches

---

- [ ] 12. CLI: Grep Auto-Uses Index + --no-index Flag

  **What to do**:
  - Modify `crates/core/commands/grep.rs`:
    ```rust
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
        // NEW: --no-index flag (default is to use index)
        #[serde(default)]
        #[arg(long)]
        pub no_index: bool,
    }
    
    pub fn run<W: Write, E: Write>(
        ctx: &mut CommandContext<'_, W, E>,
        cmd: GrepCmd,
    ) -> Result<(), LinehashError> {
        let doc = Document::load(&cmd.file)?;
        
        let lines = if cmd.no_index {
            // Force linear scan
            grep_lines(&doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
        } else {
            // AUTO use index (default behavior)
            match session.get_or_build_trigram_index(&cmd.file) {
                Ok(Some((index, meta))) => {
                    indexed_grep(&index, &doc.lines, &cmd.pattern, cmd.case_insensitive)?
                }
                Ok(None) | Err(_) => {
                    // Fallback to linear
                    grep_lines(&doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
                }
            }
        };
        
        // ... output ...
    }
    ```
  - Add `--no-index` flag to GrepCmd in cli.rs
  - Default behavior is to use index (auto-index)
  - Graceful fallback if index unavailable

---

- [ ] 13. MCP: tool_grep Uses Index Automatically

  **What to do**:
  - Modify `crates/core/mcp.rs`:
    ```rust
    // Add to SessionState
    struct SessionState {
        docs: HashMap<PathBuf, CacheEntry>,
        trigram_indexes: HashMap<PathBuf, (MmapIndex, IndexMeta)>,  // NEW
    }
    
    fn tool_grep(arguments: &Value, session: &mut SessionState) -> Result<Value, JsonRpcError> {
        let cmd: GrepCmd = parse_args(arguments)?;
        let entry = session.get(&cmd.file)?;
        
        // NEW: Try to use trigram index
        let search_result = if !cmd.no_index {
            match session.get_trigram_index(&cmd.file) {
                Ok(Some((index, _meta))) => {
                    indexed_grep(&index, &entry.doc.lines, &cmd.pattern, cmd.case_insensitive)?
                }
                _ => grep_lines(&entry.doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
            }
        } else {
            grep_lines(&entry.doc, &cmd.pattern, cmd.invert, cmd.case_insensitive)?
        };
        
        // Return success_payload with search_result
    }
    
    impl SessionState {
        fn get_trigram_index(&mut self, path: &Path) -> Result<Option<&(MmapIndex, IndexMeta)>, SearchError> {
            // Check if we have a valid cached index
            // If not, try to load from disk
            // If stale, rebuild
        }
    }
    ```
  - Session-level caching for trigram indexes (similar to doc cache)
  - Index cached with metadata for validity checking
  - Graceful fallback to linear grep on error

---

- [ ] 14. Incremental Index Updates

  **What to do**:
  - Extend `crates/core/search/persist.rs`:
    ```rust
    impl TrigramIndex {
        /// Check if index needs rebuild
        pub fn needs_rebuild(file_path: &Path, current_meta: &FileMeta) -> Result<bool, SearchError> {
            if let Some((_, meta)) = Self::load(file_path)? {
                Ok(meta.file_mtime != current_meta.mtime 
                   || meta.file_size != current_meta.size
                   || meta.content_hash != compute_content_hash(file_path)?)
            } else {
                Ok(true)  // No index exists
            }
        }
        
        /// Rebuild index if stale
        pub fn load_or_rebuild(file_path: &Path) -> Result<MmapIndex, SearchError>;
    }
    ```
  - Compare mtime, size, and content_hash for validity
  - Auto-rebuild on file changes

---

- [ ] 15. Index Stats and Diagnostics

  **What to do**:
  - Extend `crates/core/search/` with stats:
    ```rust
    pub struct IndexStats {
        pub file_path: String,
        pub line_count: usize,
        pub trigram_count: usize,
        pub unique_trigrams: usize,
        pub avg_postings_per_trigram: f64,
        pub memory_size_bytes: usize,
        pub is_valid: bool,
    }
    
    impl MmapIndex {
        pub fn stats(&self) -> IndexStats;
    }
    ```
  - Enhance `linehash index --stats` to show detailed info

---

- [ ] 16. Performance Benchmarks

  **What to do**:
  - Create `crates/core/benches/grep_bench.rs`:
    ```rust
    use criterion::{black_box, criterion_group, Criterion};
    
    fn bench_indexed_vs_linear(c: &mut Criterion) {
        // Benchmark linear grep
        // Benchmark indexed grep (with warm index)
        // Compare results
    }
    ```
  - Use `criterion` crate
  - Test multiple patterns: common, rare, regex
  - Document results

---

- [ ] 17. Documentation and Examples

  **What to do**:
  - Update `README.md`:
    ```markdown
    ## Instant Grep
    
    linehash automatically builds a trigram index when you read files,
    making grep searches fast even on large files.
    
    ```bash
    # Read a file (triggers background index build)
    linehash read large_file.rs
    
    # Grep automatically uses the index
    linehash grep large_file.rs "fn process"
    
    # Force linear scan if needed
    linehash grep large_file.rs "pattern" --no-index
    ```
    ```
  - Create `OPTIMIZE_V4.md` with architecture documentation

---

## Final Verification Wave

> 4 review agents run in PARALLEL after ALL implementation tasks.

- [ ] F1. **Plan Compliance Audit** — `oracle`
- [ ] F2. **Code Quality Review** — `unspecified-high`
- [ ] F3. **Real Manual QA** — `unspecified-high`
- [ ] F4. **Scope Fidelity Check** — `deep`

---

## Success Criteria

### Verification Commands
```bash
# Read triggers background index build
cargo build --release
./target/release/linehash read Cargo.toml

# Grep uses index automatically (check logs or timing)
./target/release/linehash grep Cargo.toml "pub fn" > /tmp/linear.txt  # falls back since no index yet
./target/release/linehash grep Cargo.toml "pub fn"  # uses index if available

# Force linear scan
./target/release/linehash grep --no-index Cargo.toml "pub fn"

# Stats work
./target/release/linehash index --stats Cargo.toml

# MCP test
echo '{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"linehash_grep","arguments":{"file":"Cargo.toml","pattern":"pub fn"}}}' | ./target/release/linehash mcp

# All tests pass
cargo test --package linehash-core
```

### Final Checklist
- [ ] All "Must Have" items present
- [ ] All "Must NOT Have" items absent
- [ ] All tests pass
- [ ] Indexed grep matches linear grep exactly
- [ ] Performance improvement measurable
- [ ] MCP integration works
- [ ] Documentation complete
- [ ] No breaking changes to existing functionality

---

## Appendix: Cursor Instant Grep Reference

### Core Algorithm
1. **Trigram Extraction**: Every 3-byte overlapping sequence becomes an index key
2. **Inverted Index**: trigram → [(line_idx, loc_mask, next_mask)]
3. **Query Decomposition**: Regex → required trigram sets (AND/OR tree)
4. **Candidate Filtering**: Use masks to filter candidates before full regex
5. **Verification**: Full regex on candidates only

### Performance Target
- Index build: O(n) — one-time cost
- Query: O(trigram_lookup + candidate_verify) — sub-millisecond for selective queries
- Speedup: 20-100× for typical code search patterns
