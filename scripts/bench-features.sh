#!/usr/bin/env bash
# Real-feature benchmark for hashline.
#
# Generates synthetic 100 / 10 000 / 100 000-line Rust source files, then runs
# `hyperfine` on each public subcommand and prints one tab-separated row
# (label, mean_ms, min_ms, max_ms) per benchmark to stdout.
#
# Requirements on PATH: hyperfine, rg, python3.
# Env knobs:
#   HASHLINE_BIN  - hashline binary to bench (default: target/release/hashline)
#   FX_DIR        - fixture directory (default: /tmp/lh-bench)
#   REPO_ROOT     - real-source fixture (default: <repo>/crates/core)
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${HASHLINE_BIN:-$ROOT/target/release/hashline}"
FX="${FX_DIR:-/tmp/lh-bench}"
REPO="${REPO_ROOT:-$ROOT/crates/core}"

for tool in hyperfine rg python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 1; }
done
[ -x "$BIN" ] || { echo "hashline binary not found at $BIN (build with: cargo build --release)" >&2; exit 1; }

mkdir -p "$FX"
python3 - <<PY
import random, string, os
random.seed(0)
def gen(n, path):
    if os.path.exists(path) and os.path.getsize(path) > 0:
        return
    lines = []
    for i in range(n):
        token = ''.join(random.choices(string.ascii_lowercase, k=random.randint(4,12)))
        lines.append(f'pub fn func_{i}_{token}(arg_{i}: i32) -> i32 {{ arg_{i} + {i} }}')
    open(path,'w').write('\n'.join(lines)+'\n')
gen(100,           "$FX/small.rs")
gen(10_000,        "$FX/medium.rs")
gen(100_000,       "$FX/large.rs")
PY

pick_anchor() {
  local file="$1" line="$2"
  "$BIN" index "$file" 2>/dev/null | awk -v ln="$line" 'NR==ln{print $1}'
}

A_SMALL=$(pick_anchor "$FX/small.rs" 50)
A_MED=$(pick_anchor "$FX/medium.rs" 5000)
A_LARGE=$(pick_anchor "$FX/large.rs" 50000)
RANGE_LARGE_S=$(pick_anchor "$FX/large.rs" 40000)
RANGE_LARGE_E=$(pick_anchor "$FX/large.rs" 60000)

emit() {
  local label="$1"
  local mean min max
  mean=$(python3 -c "import json; r=json.load(open('/tmp/hf.json'))['results'][0]; print(f\"{r['mean']*1000:.2f}\")")
  min=$(python3 -c "import json; r=json.load(open('/tmp/hf.json'))['results'][0]; print(f\"{r['min']*1000:.2f}\")")
  max=$(python3 -c "import json; r=json.load(open('/tmp/hf.json'))['results'][0]; print(f\"{r['max']*1000:.2f}\")")
  printf '%s\t%s\t%s\t%s\n' "$label" "$mean" "$min" "$max"
}

run() {
  local label="$1"; shift
  local cmd="$*"
  echo "## $label" >&2
  if hyperfine --warmup 2 --runs 5 --export-json /tmp/hf.json "$cmd" >/dev/null 2>&1 \
     || hyperfine --warmup 1 --runs 3 --export-json /tmp/hf.json "$cmd" >/dev/null 2>&1; then
    emit "$label"
  else
    echo "FAIL: $cmd" >&2
  fi
}

mutate() {
  local label="$1" src="$2" tgt="$3"; shift 3
  echo "## $label" >&2
  if hyperfine --warmup 1 --runs 5 --prepare "cp $src $tgt" --export-json /tmp/hf.json "$@" >/dev/null 2>&1; then
    emit "$label"
  else
    echo "FAIL: $*" >&2
  fi
}

# --- READ ---
run "read · small (100 L)"            "$BIN read $FX/small.rs"
run "read · medium (10k L)"           "$BIN read $FX/medium.rs"
run "read · large (100k L)"           "$BIN read $FX/large.rs"
run "read --json · large (100k L)"    "$BIN read $FX/large.rs --json"
run "read --anchor+ctx · large"       "$BIN read $FX/large.rs --anchor $A_LARGE --context 5"

# --- INDEX ---
run "index · small"                   "$BIN index $FX/small.rs"
run "index · medium"                  "$BIN index $FX/medium.rs"
run "index · large"                   "$BIN index $FX/large.rs"

# --- VERIFY ---
run "verify · 1 anchor · large"       "$BIN verify $FX/large.rs $A_LARGE"
run "verify · 10 anchors · large"     "$BIN verify $FX/large.rs $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE $A_LARGE"

# --- GREP variants + rg baseline ---
hyperfine --warmup 1 --runs 5 --prepare 'rm -rf /tmp/lh-cache && mkdir -p /tmp/lh-cache' \
  --export-json /tmp/hf.json \
  "env XDG_CACHE_HOME=/tmp/lh-cache $BIN grep $FX/large.rs func_50000" >/dev/null 2>&1 \
  && emit "grep · large · trigram (cold)" || echo "FAIL: grep cold" >&2

run "grep · large · trigram (warm)"   "env XDG_CACHE_HOME=/tmp/lh-cache $BIN grep $FX/large.rs func_50000"
run "grep · large · --no-index"       "$BIN grep --no-index $FX/large.rs func_50000"
run "rg · large (baseline)"           "rg --no-config -n func_50000 $FX/large.rs"

# Daemon: warm only. The cold variant (auto-spawn) can block in some sandboxes.
pkill -f 'hashline daemon' 2>/dev/null; sleep 0.5
"$BIN" daemon >/dev/null 2>&1 &
DPID=$!
sleep 1
run "grep · large · daemon (warm)"    "$BIN grep $FX/large.rs func_50000 --daemon"
kill $DPID 2>/dev/null
pkill -f 'hashline daemon' 2>/dev/null

# --- ANNOTATE ---
run "annotate · large · substring"    "$BIN annotate $FX/large.rs func_50000"

# --- MUTATIONS ---
mutate "edit · small · 1 line"     "$FX/small.rs"  "$FX/m_small.rs"  "$BIN edit $FX/m_small.rs $A_SMALL replaced"
mutate "edit · medium · 1 line"    "$FX/medium.rs" "$FX/m_medium.rs" "$BIN edit $FX/m_medium.rs $A_MED replaced"
mutate "edit · large · 1 line"     "$FX/large.rs"  "$FX/m_large.rs"  "$BIN edit $FX/m_large.rs $A_LARGE replaced"
mutate "edit · large · range 2k L" "$FX/large.rs"  "$FX/m_large.rs"  "$BIN edit $FX/m_large.rs ${RANGE_LARGE_S}..${RANGE_LARGE_E} merged"
mutate "insert · large"            "$FX/large.rs"  "$FX/m_large.rs"  "$BIN insert $FX/m_large.rs $A_LARGE inserted"
mutate "delete · large"            "$FX/large.rs"  "$FX/m_large.rs"  "$BIN delete $FX/m_large.rs $A_LARGE"
mutate "swap · large"              "$FX/large.rs"  "$FX/m_large.rs"  "$BIN swap $FX/m_large.rs $A_LARGE $RANGE_LARGE_S"
mutate "move · large"              "$FX/large.rs"  "$FX/m_large.rs"  "$BIN move $FX/m_large.rs $A_LARGE after $RANGE_LARGE_S"
mutate "indent · large · range"    "$FX/large.rs"  "$FX/m_large.rs"  "$BIN indent $FX/m_large.rs ${RANGE_LARGE_S}..${RANGE_LARGE_E} +2"

# --- PATCH (10 ops) ---
python3 - <<PY
import json, subprocess
out = subprocess.check_output(['$BIN','index','$FX/large.rs']).decode().splitlines()
ops = []
for ln in [1000,5000,10000,20000,30000,40000,50000,60000,70000,80000]:
    a = out[ln-1].split()[0]
    ops.append({'op':'edit','anchor':a,'content':f'patched_{ln}'})
open('$FX/patch.json','w').write(json.dumps({'ops':ops}))
PY
mutate "patch · large · 10 ops"    "$FX/large.rs"  "$FX/m_large.rs"  "$BIN patch $FX/m_large.rs $FX/patch.json"

# --- BLOCK / DIAGNOSTICS ---
run "find-block · large"              "$BIN find-block $FX/large.rs $A_LARGE"
run "stats · large"                   "$BIN stats $FX/large.rs"
run "doctor · large"                  "$BIN doctor $FX/large.rs"

# --- MAP / LANGUAGE TOOLS (real source) ---
run "map · core/ (real repo)"         "$BIN map $REPO --json"
run "outline · cli.rs"                "$BIN outline $REPO/cli.rs"
run "outline · context.rs"            "$BIN outline $REPO/context.rs"
run "symbol · 'EditCmd' --scope core" "$BIN symbol EditCmd --scope $REPO --json"
run "callers · 'parse_anchor'"        "$BIN callers parse_anchor --scope $REPO --depth 3 --json"
run "callees · 'run' --depth 2"       "$BIN callees run --scope $REPO --depth 2 --json"
run "deps · cli.rs"                   "$BIN deps --file $REPO/cli.rs --json"

# --- MISC ---
run "workflows · --root core"         "$BIN workflows --root $REPO/.."
run "watch-capabilities --json"       "$BIN watch-capabilities --json"

echo "DONE" >&2
