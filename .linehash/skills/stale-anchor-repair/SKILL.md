---
title = "Stale Anchor Repair"
description = "Recover from stale or ambiguous anchors without guessing and without bypassing the safety system."
surfaces = ["local", "mcp"]
allowed_cli_commands = ["linehash read", "linehash verify", "linehash doctor"]
allowed_mcp_tools = ["linehash_read", "linehash_verify", "linehash_doctor"]
tags = ["stale-anchor", "recovery", "safety"]
---
Use this pack when a mutation is rejected because anchors drifted or collided.

Recovery loop:
1. Treat the rejection as a valid safety stop.
2. Re-read the file or the reported neighborhood to rebuild a fresh qualified anchor.
3. Re-run `verify` before retrying a grouped edit.
4. Use `doctor` when you need guidance on whether to switch to patch or block-oriented flow.
