# Planning Spec: DEL.BLK Leaves Orphan Blank Line

| Field | Value |
|-------|-------|
| Issue | [#79](https://github.com/quangdang46/hashline/issues/79) — DEL.BLK leaves orphan blank line |
| Date | 2026-06-21 |
| Author | Agent (hermes) |
| Status | Implemented |
| Version | 2 |

## Problem

`DEL.BLK` deletes a syntactic block but leaves the trailing blank line that separated
it from the next block. This forces agents to issue a second `DEL` patch to clean up
— doubling the operations for what should be one action.

### Reproduction (Python)

```python
def alpha():
    pass

def beta():
    pass
```

`hashline patch test.py 'DEL.BLK 1'` produces:

```
1:05|         ← orphan blank line — not part of any remaining block
2:17|def beta():
3:c1|    pass
```

### Reproduction (Rust, brace-delimited)

```rust
fn alpha() {
}

fn beta() {
}
```

`hashline patch test.rs 'DEL.BLK 1'` produces:

```
1:05|         ← same orphan blank line
2:17|fn beta() {
3:c1|}
```

The blank line between the two blocks survived deletion in both cases.

## Root Cause

There are **two independent flaws** that contribute to the bug, both in
`crates/core/src/commands/patch.rs`:

### Flaw 1: `find_block_from_header` skips nothing when scanning forward

[Lines 632–646] The function scans forward from the block header to find the
next same-or-less-indented line, which marks where the block ends. Unlike
`find_block_from_body` (which skips blank lines), the header variant does NOT
skip blank lines — so a blank line at the same indent immediately terminates the
block span, pushing the blank line *outside* the deletion range.

Compare the two forward scan loops:

| Function | Skips blank lines? | Skips comments? |
|----------|-------------------|-----------------|
| `find_block_from_header` (patch.rs:632) | No | No |
| `find_block_from_body` (patch.rs:649) | Yes | No |
| `find_python_block` (find_block.rs:392) | Yes | Yes (`#` lines) |

The header variant is used for all top-level blocks (`def`, `class`, `fn`, etc.
at indent 0). The body variant is used for nested blocks. This inconsistency
means a top-level `def` in Python resolves to a narrower span than an `if`
nested inside it.

### Flaw 2: DEL.BLK has no trailing-blank cleanup

Even after the block span is correctly resolved, the DELETE step at [lines
448–452] removes only the span lines. No logic consumes adjacent blank lines
after the deletion range, so any blank that happened to follow the block (e.g.
between `}` and `fn` in a brace-language file) remains as an orphan.

Flaw 1 affects indentation-based languages (Python, Verse, generic indent).
Flaw 2 affects all languages including brace-delimited (C, Rust, JS, Go, etc.).

## Goals

1. **DEL.BLK must consume the trailing blank line before the next block** as its
   default behaviour, eliminating the need for a follow-up `DEL` patch.
2. Maintain the existing safety invariants: stale-anchor detection, concurrency
   safety, no fuzzy matching.
3. Zero behaviour change for blocks that already end at EOF (no blank line
   after them) — no spurious deletions.
4. Keep the fix minimal: no new CLI flags, no structural refactors.

## Approach

Fix Flaw 1 **and** Flaw 2 — each is independently necessary:

### Fix 1: Skip blank lines in `find_block_from_header` forward scan

**File:** `crates/core/src/commands/patch.rs` (line ~632)

Make the forward scan in `find_block_from_header` skip lines whose `trim()`
is empty, matching the existing behaviour of `find_block_from_body`.

```rust
// Before:
if leading_ws(&entries[i].content) <= si {
    end = i.saturating_sub(1);
    break;
}

// After:
if entries[i].content.trim().is_empty() {
    continue;
}
if leading_ws(&entries[i].content) <= si {
    end = i.saturating_sub(1);
    break;
}
```

**Effect:** For the Python reproduction case, the block span expands from
`(0, 1)` to `(0, 2)` — the blank line between `alpha()` and `beta()` is now
included in the deletion range, so it gets removed along with the function body.

### Fix 2: Consume trailing blank lines after DEL.BLK

**File:** `crates/core/src/commands/patch.rs` (line ~450)

In the `DEL.BLK` branch (the `None if payloads.is_empty()` arm), after the
existing removal loop, consume any immediately following blank lines:

```rust
// After the existing removal loop:
// Consume trailing blank lines after deleted block
while block_start < lines.len() && lines[block_start].trim().is_empty() {
    lines.remove(block_start);
}
```

**Effect:** For brace-delimited languages (where the brace pair is exact and
blanks after `}` are outside the pair), the post-deletion cleanup removes the
orphan blank. This also serves as a safety net for any indentation-based
language case where a non-blank boundary comment precedes the next block.

### Interaction of the Two Fixes

| Scenario | Fix 1 helps? | Fix 2 helps? | Result |
|----------|-------------|-------------|--------|
| Python `def a():` followed by blank line then `def b():` | Yes (blank included in span) | Yes (safety net) | Clean |
| Rust `fn a() {}` followed by blank line then `fn b() {}` | No (brace pair is exact) | Yes (post-deletion cleanup) | Clean |
| Python `def a():` at end of file | No (EOF reached, no blank to skip) | N/A | No change |
| Rust `fn a() {}` at end of file | N/A | N/A | No change |

Both fixes land the same result: the orphan blank is consumed.

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Blank line between block and following content is semantically meaningful (e.g. intentional spacing) | Low — blank lines between top-level constructs are visual separators, not content | The fix treats them as separators that belong to the preceding block, matching the UX intuition of "delete this function and the gap it left." |
| Blank line following a deleted block was meant to be preserved for follow-up edits (insert-after-blank workflow) | Low — INS.BLK.POST exists for inserting after a block; the blank was never a reliable insertion anchor (shift-prone) | If preserving the blank is critical, the user can insert it back via `INS.POST` — far cheaper than having all agents pay an extra `DEL` call on every block deletion. |
| Post-deletion cleanup eats too many blank lines | Low | The `while` loop only removes immediately adjacent blank lines. As soon as a non-blank line is encountered, the loop stops. Multiple blank lines between functions (rare in practice) are all consumed, which is correct — they were all part of the visual gap. |

## Tests

### New unit tests in `patch.rs` (`#[cfg(test)]`)

1. **`test_delete_block_python_with_trailing_blank`** — Python file with two
   functions separated by a blank line. `DEL.BLK 1` should delete only the
   first function's block and erase the trailing blank, leaving `def beta():`
   at line 1.

2. **`test_delete_block_rust_with_trailing_blank`** — Rust file with two
   brace-delimited functions separated by a blank line. `DEL.BLK 1` should
   delete the first function and the trailing blank, leaving `fn beta()` at
   line 1.

3. **`test_delete_block_at_eof_noop`** — Python/Rust file with a single
   function (no trailing blank and no next function). `DEL.BLK 1` should
   delete it cleanly, producing an empty file or EOF.  (This test should
   already pass, included as a regression guard.)

4. **`test_delete_block_multiple_blanks_between`** — Two functions separated by
   two blank lines. `DEL.BLK 1` should consume both blank lines, leaving
   just the second function.

### Acceptance criteria

All existing tests pass with no modification required to inline assertions.
The new tests above pass with the fix and fail (demonstrating the bug) without it.

## Implementation Order

1. Fix 1: Add blank-line skip to `find_block_from_header` forward scan.
2. Fix 2: Add trailing-blank cleanup after DEL.BLK removal loop.
3. Add new unit tests.
4. Run `cargo test` to confirm all tests pass.
5. Commit.

## Validation

```bash
cargo test  # Must pass all existing and new tests
```

No config, CLI, or documentation changes needed — this is a pure behavioural
fix that makes `DEL.BLK` match user expectations.

## Implementation

The fix was implemented in commit `87834a7` (PR [#82](https://github.com/quangdang46/hashline/pull/82)):

| Item | Status |
|------|--------|
| Fix 1: blank-line skip in `find_block_from_header` | ✅ Merged in `patch.rs` |
| Fix 2: trailing-blank cleanup after DEL.BLK | ✅ Merged in `patch.rs` |
| `test_delete_block_python_with_trailing_blank` | ✅ Passes |
| `test_delete_block_rust_with_trailing_blank` | ✅ Passes |
| `test_delete_block_at_eof_noop` | ✅ Passes (regression guard) |
| `test_delete_block_multiple_blanks_between` | ✅ Passes |
| All existing tests (200+ total) | ✅ All pass with `cargo test` |
