---
title = "Patch Transaction"
description = "Bundle several coordinated anchor mutations into one reviewable transaction."
surfaces = ["local", "mcp"]
allowed_cli_commands = ["linehash annotate", "linehash grep", "linehash find-block", "linehash patch", "linehash merge-patches", "linehash from-diff"]
allowed_mcp_tools = ["linehash_annotate", "linehash_grep", "linehash_find_block", "linehash_patch", "linehash_merge_patches", "linehash_from_diff"]
tags = ["patch", "transaction", "multi-step"]
---
Use this pack when multiple edits must succeed or fail together.

Workflow:
1. Gather anchors with `find-block`, `annotate`, or `grep`.
2. Build a patch file and dry-run it first.
3. Prefer `merge-patches` when combining independent change sets.
4. Convert unified diffs with `from-diff` only when a review artifact already exists.
