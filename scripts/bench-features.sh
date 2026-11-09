#!/usr/bin/env bash
# hashline feature benchmark — thin wrapper around scripts/bench.py
# (cross-platform: no hyperfine/sed dependency, works on Windows + Linux).
#
# Usage:
#   scripts/bench-features.sh            # writes bench-results/bench-YYYY-MM-DD.md
#   HASHLINE_BIN=./target/release/hashline scripts/bench-features.sh
#
# Env:
#   HASHLINE_BIN  binary to bench (default: target/release/hashline[.exe])
#   FX_DIR        fixture directory (default: <temp>/hashline-bench)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/bench.py" "$@"
