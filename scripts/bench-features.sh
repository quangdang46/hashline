#!/usr/bin/env bash
# hashline feature benchmark — outputs a single Markdown report.
#
# Usage:
#   scripts/bench-features.sh                    # writes bench-results/bench-YYYY-MM-DD.md
#   HASHLINE_BIN=./target/release/hashline scripts/bench-features.sh
#
# Requirements: hyperfine, python3
# Env:
#   HASHLINE_BIN  binary to bench (default: target/release/hashline)
#   FX_DIR        fixture directory (default: /tmp/hashline-bench)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${HASHLINE_BIN:-$ROOT/target/release/hashline}"
FX="${FX_DIR:-/tmp/hashline-bench}"
OUT="$ROOT/bench-results/bench-$(date +%Y-%m-%d).md"

for tool in hyperfine python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing: $tool" >&2; exit 1; }
done
[ -x "$BIN" ] || { echo "binary not found: $BIN  (run: cargo build --release)" >&2; exit 1; }

mkdir -p "$FX" "$ROOT/bench-results"

# ── Generate fixtures ────────────────────────────────────────────────────────
python3 - <<PY
import random, os
random.seed(0)
def gen(n, path):
    if os.path.exists(path) and os.path.getsize(path) > 0:
        return
    lines = [f'pub fn func_{i}(x: i32) -> i32 {{ x + {i} }}' for i in range(n)]
    open(path, 'w').write('\n'.join(lines) + '\n')
gen(100,     "$FX/small.rs")
gen(10_000,  "$FX/medium.rs")
gen(100_000, "$FX/large.rs")
print("fixtures ready")
PY

# ── Pick anchors ─────────────────────────────────────────────────────────────
pick() { "$BIN" index "$1" 2>/dev/null | awk -v n="$2" 'NR==n{print $1}'; }
A_SMALL=$(pick "$FX/small.rs"  50)
A_MED=$(pick   "$FX/medium.rs" 5000)
A_LARGE=$(pick "$FX/large.rs"  50000)
A_LARGE2=$(pick "$FX/large.rs" 50010)

# ── Helpers ──────────────────────────────────────────────────────────────────
HF_JSON=/tmp/hashline-hf.json

hf_run() {
  hyperfine --warmup 2 --runs 5 --export-json "$HF_JSON" "$@" >/dev/null 2>&1 \
  || hyperfine --warmup 1 --runs 3 --export-json "$HF_JSON" "$@" >/dev/null 2>&1
}

hf_mutate() {
  local src="$1" dst="$2"; shift 2
  hyperfine --warmup 1 --runs 5 \
    --prepare "cp $src $dst" \
    --export-json "$HF_JSON" "$@" >/dev/null 2>&1 \
  || hyperfine --warmup 1 --runs 3 \
    --prepare "cp $src $dst" \
    --export-json "$HF_JSON" "$@" >/dev/null 2>&1
}

# Read mean/min/max from last hyperfine JSON, print as "| label | Xms | X–Xms |"
row() {
  local label="$1"
  python3 - "$label" <<'PY'
import json, sys
r = json.load(open("/tmp/hashline-hf.json"))["results"][0]
mean = r["mean"] * 1000
lo   = r["min"]  * 1000
hi   = r["max"]  * 1000
print(f"| {sys.argv[1]} | {mean:.1f} ms | {lo:.1f}–{hi:.1f} ms |")
PY
}

FAIL() { echo "| $1 | FAIL | — |"; }

bench() {
  local label="$1"; shift
  echo "  $label" >&2
  if hf_run "$@"; then row "$label"; else FAIL "$label"; fi
}

mutbench() {
  local label="$1" src="$2" dst="$3"; shift 3
  echo "  $label" >&2
  if hf_mutate "$src" "$dst" "$@"; then row "$label"; else FAIL "$label"; fi
}

# ── Report ───────────────────────────────────────────────────────────────────
{
VERSION=$("$BIN" --version 2>/dev/null || echo "unknown")
DATE=$(date +"%Y-%m-%d %H:%M")
HOST=$(uname -srm)

cat <<MD
# hashline benchmark — $DATE

**Binary:** \`$BIN\`  
**Version:** $VERSION  
**Host:** $HOST  
**Fixtures:** small=100L, medium=10kL, large=100kL

---

## Read & orient

| Command | Mean | Range |
|---------|-----:|------:|
MD

echo "=== Read & orient ===" >&2
bench "read small.rs (100 L)"         "$BIN read $FX/small.rs"
bench "read medium.rs (10k L)"        "$BIN read $FX/medium.rs"
bench "read large.rs (100k L)"        "$BIN read $FX/large.rs"
bench "read large.rs --json"          "$BIN read $FX/large.rs --json"
bench "read large.rs --anchor+ctx"    "$BIN read $FX/large.rs --anchor $A_LARGE --context 5"
bench "index large.rs"                "$BIN index $FX/large.rs"

cat <<MD

## Verify

| Command | Mean | Range |
|---------|-----:|------:|
MD

echo "=== Verify ===" >&2
bench "verify large.rs 1 anchor"      "$BIN verify $FX/large.rs $A_LARGE"
bench "verify large.rs 10 anchors"    "$BIN verify $FX/large.rs $(printf "$A_LARGE %.0s" {1..10})"

cat <<MD

## Mutations

> Single-line \`edit\` uses an mmap fast-path; all others rewrite the whole file via atomic-rename.

| Command | Mean | Range |
|---------|-----:|------:|
MD

echo "=== Mutations ===" >&2
mutbench "edit small.rs 1 line"    "$FX/small.rs"  "$FX/m.rs" "$BIN edit $FX/m.rs $A_SMALL replaced"
mutbench "edit medium.rs 1 line"   "$FX/medium.rs" "$FX/m.rs" "$BIN edit $FX/m.rs $A_MED replaced"
mutbench "edit large.rs 1 line"    "$FX/large.rs"  "$FX/m.rs" "$BIN edit $FX/m.rs $A_LARGE replaced"
mutbench "insert large.rs"         "$FX/large.rs"  "$FX/m.rs" "$BIN insert $FX/m.rs $A_LARGE inserted"
mutbench "delete large.rs"         "$FX/large.rs"  "$FX/m.rs" "$BIN delete $FX/m.rs $A_LARGE"
mutbench "swap large.rs"           "$FX/large.rs"  "$FX/m.rs" "$BIN swap $FX/m.rs $A_LARGE $A_LARGE2"
mutbench "move large.rs"           "$FX/large.rs"  "$FX/m.rs" "$BIN move $FX/m.rs $A_LARGE after $A_LARGE2"
mutbench "indent large.rs range"   "$FX/large.rs"  "$FX/m.rs" "$BIN indent $FX/m.rs ${A_LARGE}..${A_LARGE2} +2"

# patch: 10 ops
python3 - <<PY
import json, subprocess
lines = subprocess.check_output(['$BIN','index','$FX/large.rs']).decode().splitlines()
ops = [{'op':'edit','anchor':lines[ln-1].split()[0],'content':f'p{ln}'}
       for ln in [1000,5000,10000,20000,30000,40000,50000,60000,70000,80000]]
open('$FX/patch.json','w').write(json.dumps({'ops':ops}))
PY
mutbench "patch large.rs 10 ops"   "$FX/large.rs"  "$FX/m.rs" "$BIN patch $FX/m.rs $FX/patch.json"

cat <<MD

## Diagnostics

| Command | Mean | Range |
|---------|-----:|------:|
MD

echo "=== Diagnostics ===" >&2
bench "stats large.rs"   "$BIN stats $FX/large.rs"
bench "doctor large.rs"  "$BIN doctor $FX/large.rs"

echo ""
echo "---"
echo ""
echo "_Generated by \`scripts/bench-features.sh\` — $(date)_"

} | tee "$OUT"

echo "" >&2
echo "Report written to: $OUT" >&2
