---
title = "Anchored Read"
description = "Orient on a file, localize the target, and pull only the minimum snippet needed before editing."
surfaces = ["local", "mcp"]
allowed_cli_commands = ["linehash index", "linehash grep", "linehash annotate", "linehash read", "linehash find-block"]
allowed_mcp_tools = ["linehash_index", "linehash_grep", "linehash_annotate", "linehash_read", "linehash_find_block"]
tags = ["read", "orientation", "snippets"]
---
Use this pack when you need anchors before planning a change.

Preferred sequence:
1. Start with `linehash index` for noisy or large files.
2. Use `linehash grep` or `linehash annotate` when you know target text.
3. Finish with `linehash read --anchor ... --context N` instead of a repeated full-file dump.

Keep context windows tight. Escalate to `find-block` only when one snippet is not enough.
