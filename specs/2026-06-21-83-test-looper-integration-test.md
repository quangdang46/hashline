# Planning Spec: Looper Integration Test for Hashline

| Field | Value |
|-------|-------|
| Issue | [#83](https://github.com/quangdang46/hashline/issues/83) — TEST: Looper integration test for hashline |
| Date | 2026-06-21 |
| Author | Agent (hermes) |
| Status | Draft |
| Version | 1 |

## Problem

[Looper](https://github.com/nexu-io/looper) is an autonomous multi-agent dev
system that picks up GitHub issues, produces planning specs, generates code,
reviews its own output, and opens PRs. The hashline repository is an early
adopter of Looper's planner/reviewer/fixer/worker lifecycle, but there is no
formal integration test that exercises the **full end-to-end Looper lifecycle**
on this repo.

Without such a test, regressions in Looper's behavior (issue parsing, label
selection, branch naming, spec generation, review loop, PR creation) go
undetected until a human notices a broken workflow.

### What We're Actually Testing

This is a **meta-test**: the artifact produced is not a new feature in the
hashline CLI but a reproducible demonstration that Looper can navigate the
hashline repository correctly through its entire pipeline:

1. **Issue pick-up** — Looper selects issues labeled `looper:plan`
2. **Spec generation** — The planner agent writes a `.md` spec into `specs/`
3. **PR creation** — A spec PR is opened on the `looper/planner/*` branch
4. **Review loop** — The reviewer agent inspects the spec and approves it
5. **Implementation** — The fixer/worker agents (optionally) write code
6. **CI gating** — `cargo test` must pass before merge

## Goals

1. **Verifiable end-to-end Looper lifecycle** — A repeatable test that
   exercises every stage of Looper's pipeline on the hashline repo, producing
   observable artifacts (spec file, commits, PR, CI runs).

2. **Regression detection** — If a future Looper or hashline change breaks
   the lifecycle, running the same test should fail in a detectable way
   (e.g., no PR created, CI fails, spec file absent).

3. **Minimal permanent footprint** — The test should leave only:
   - A planning spec in `specs/` (this file)
   - A closed issue with observable PR history
   - Zero production-code changes to hashline itself

## Approach

### Phase 1: Planning spec (current phase)

The planner agent (this run) creates a spec document in `specs/`, commits it
on the lifecycle branch `looper/planner/83-test-looper-integration-test`, and
opens a PR against `main`.

**Artifact:** This spec file.

### Phase 2: Spec review

The reviewer agent loads the spec, inspects it for correctness, clarity, and
alignment with the issue body, then approves or requests changes. The review
must confirm:

- The spec file exists at the expected path
- It references the correct issue URL and number
- It describes the problem, approach, risks, and validation
- No implementation scope leak (no code changes outside `specs/`)

**Artifact:** Approved PR with reviewer comment.

### Phase 3: Implementation

The fixer agent or worker agent executes the plan (in this case, the "fix" is
the test infrastructure itself). Because this is a meta-test, the
"implementation" is:

1. Verifying the spec PR exists and is approved
2. Running `cargo test --workspace --all-features` to confirm hashline still
   compiles and passes all tests (the test must not break anything)
3. Performing any structural changes required for the Looper lifecycle itself
   (e.g., label management, branch cleanup)

**Artifact:** Merge-ready PR with passing CI.

### Phase 4: CI gate

After merge, the CI pipeline (`.github/workflows/ci.yml`) runs
`cargo test --workspace --all-features --locked` across three platforms.

**Validation checkpoint:** CI must pass on the merged commit.

### Self-Proving Nature

A key design constraint: **the Looper lifecycle itself IS the test**. Each phase
produces an observable artifact that the next phase can inspect. If any phase
fails to produce its artifact, the chain breaks and the test fails with an
obvious signal (missing PR, missing file, CI red).

| Phase | Produces | Consumed by |
|-------|----------|-------------|
| Plan | `specs/2026-06-21-83-*.md` file committed + pushed | Reviewer |
| Review | PR approval comment + label | Fixer / Worker |
| Fix | Passing `cargo test` on lifecycle branch | CI gate |
| Merge | Green CI on `main` | Human or auto-merge |

## Test Scenarios

### S1: Happy-path lifecycle

1. Issue #83 exists, labeled `looper:plan`, assigned to `quangdang46`
2. Planner creates `specs/2026-06-21-83-*.md` on branch
   `looper/planner/83-test-looper-integration-test`
3. Planner opens PR against `main`
4. Reviewer inspects spec, approves
5. Fixer confirms `cargo test` passes
6. CI runs green
7. PR is merged

**Pass condition:** All seven steps complete without human intervention.

### S2: No spec file → fail fast

If the planner does not produce a spec file at the expected path, the reviewer
rejects the PR and the chain stops.

**Pass condition:** Clean failure with an explanatory comment.

### S3: Broken spec → review rejection

If the spec is missing required sections (Problem, Approach, Risks, Validation),
the reviewer requests changes.

**Pass condition:** Review comment listing missing sections.

### S4: CI regression

If the lifecycle branch introduces a change that breaks `cargo test`, the CI
gate must fail before merge.

**Pass condition:** Red CI status on the PR.

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Looper label (`looper:plan`) is removed before planning completes | Low | Issue labels are sticky; removal mid-cycle would cancel this run, which is itself a valid test outcome (observable via log) |
| Branch name collision from concurrent Looper runs | Low | Each issue number produces a unique branch name (`looper/planner/83-*`). Concurrent runs against different issues produce different branches |
| Spec file path changes between Looper versions | Medium | The spec path is derived from the issue date and number. If the convention changes, the test reveals it by producing no artefact at the expected path |
| hashline CI is already red on `main` before the test runs | Low | The test should gate on the lifecycle branch's CI, not `main`'s. Pre-existing failures would also affect S4 but that's a valid regression alert |
| Multiple agents modify the same branch concurrently | High (this repo has many worktree agents) | The spec branch is isolated per-issue. Looper's write to `specs/` is additive (new file, no conflict). Other agents' concurrent edits to source files (Cargo.toml, etc.) are not on this branch |
| The test produces no code changes, which may not trigger the CI workflow | Low | CI runs on all PRs targeting `main`, regardless of whether code changed. A PR with only a spec file change still triggers CI |

## Validation

### Immediate validation (post-spec creation)

```bash
# Spec file exists
test -f specs/2026-06-21-83-test-looper-integration-test.md

# Branch exists and has the spec commit
git log --oneline looper/planner/83-test-looper-integration-test | head -3

# PR exists
gh pr list --head looper/planner/83-test-looper-integration-test --json number,state,url

# Cargo still compiles (sanctity check — spec creation shouldn't break anything)
cargo test --workspace --all-features --locked
```

### Merge gate validation

```bash
# Reviewer approved
gh pr view --json reviews,state

# CI is green
gh pr view --json statusCheckRollup

# PR is mergeable
gh pr view --json mergeStateStatus
```

### Post-merge validation

```bash
# Issue is closed
gh issue view 83 --json state

# Spec file exists on main
git show main:specs/2026-06-21-83-test-looper-integration-test.md > /dev/null
```

## Implementation Order

This is a **Looper lifecycle test**, so the implementation is the lifecycle
itself:

1. **This run (planner):** Write spec, commit, push, open PR — **DONE** when
   this file is on the branch and a PR exists.
2. **Next phase (reviewer):** Inspect spec, approve or reject.
3. **Next phase (fixer/worker):** Validate `cargo test`, ensure PR is mergeable.
4. **Final phase (human or auto-merge):** Merge PR, close issue.

No production code in `src/`, `Cargo.toml`, or `Cargo.lock` is modified by this
integration test. The only file touched is this spec document.
