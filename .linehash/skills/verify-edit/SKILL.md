---
title = "Verify Then Edit"
description = "Reconfirm anchors before mutation, then apply the smallest safe edit surface."
surfaces = ["local", "mcp"]
allowed_cli_commands = ["linehash verify", "linehash edit", "linehash insert", "linehash delete"]
allowed_mcp_tools = ["linehash_verify", "linehash_edit", "linehash_insert", "linehash_delete"]
tags = ["mutation", "verification", "safety"]
---
Use this pack when you already know the anchor and want the narrowest possible mutation.

Workflow:
1. Run `verify` if another agent or tool may have touched the file.
2. Use `edit` for replacement, `insert` for additive changes, and `delete` for removal.
3. Re-read the edited neighborhood or re-run `verify` when the follow-up plan still depends on the same anchors.
