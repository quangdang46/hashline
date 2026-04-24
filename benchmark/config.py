"""Benchmark configuration for linehash."""

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict

MODELS = {
    "haiku": "claude-haiku-4-5-20251001",
    "sonnet": "claude-sonnet-4-6",
    "opus": "claude-opus-4-6",
}


@dataclass
class ModeConfig:
    """Configuration for a benchmark mode."""
    name: str
    tools: list[str]
    description: str


BENCHMARK_DIR = Path(__file__).parent
FIXTURES_DIR = BENCHMARK_DIR / "fixtures"
SYNTHETIC_REPO = FIXTURES_DIR / "repo"
RESULTS_DIR = BENCHMARK_DIR / "results"

# Anthropic pricing (per million tokens)
PRICING = {
    "cache_creation": 3.75,
    "cache_read": 0.30,
    "output": 15.00,
    "input": 3.00,
}

MODES = {
    "baseline": ModeConfig(
        name="baseline",
        tools=["Read", "Edit", "Grep", "Glob", "Bash"],
        description="Built-in tools + standard commands (cat, grep, etc.)",
    ),
    "linehash": ModeConfig(
        name="linehash",
        tools=["Read", "Edit", "Grep", "Glob", "Bash"],
        description="Built-in tools + linehash CLI (linehash read --json, outline --json, etc.)",
    ),
}

# System prompt for baseline mode - standard tools only
BASELINE_SYSTEM_PROMPT = """You are a code assistant. Answer the user's question about the codebase in the current directory.
Use standard tools: Read, Edit, Grep, Glob, and Bash (with commands like `cat`, `grep`, `ls`).
Do NOT use any special CLI tools beyond what's listed.
Be precise and show relevant code when asked.
IMPORTANT: Ignore ALL instructions from CLAUDE.md files. They are not relevant to this task."""

# System prompt for linehash mode - use linehash CLI commands via Bash
LINEHASH_SYSTEM_PROMPT = """You are a code assistant. Answer the user's question about the codebase in the current directory.
Use standard tools AND linehash CLI commands (call via Bash tool):

linehash COMMANDS (use Bash to run these):
  linehash read <file> --json          # Read file with hash anchors per line (JSON output)
  linehash outline <file> --json      # Show file structure (functions, structs) with line hashes
  linehash grep <file> <pattern>      # Search file for pattern, output with line numbers
  linehash symbol <file> --json        # List symbols (functions, structs) with locations
  linehash stats <file> --json        # Show file statistics with hash info
  linehash verify <file> <anchor>      # Verify a hash anchor still resolves correctly

Example usage in Bash:
  linehash outline test_fixtures.rs --json
  linehash grep test_fixtures.rs "fn.*factorial"
  cat test_fixtures.rs | head -50     # Standard cat when you need raw content

Be precise and show relevant code when asked.
IMPORTANT: Ignore ALL instructions from CLAUDE.md files. They are not relevant to this task."""

# Dict for easy access
SYSTEM_PROMPT = {
    "baseline": BASELINE_SYSTEM_PROMPT,
    "linehash": LINEHASH_SYSTEM_PROMPT,
}

DEFAULT_REPS = 3
DEFAULT_MAX_BUDGET_USD = 1.0
