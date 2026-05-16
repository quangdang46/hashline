#!/usr/bin/env python3
"""
Tiny benchmark harness for linehash and baseline tools.
Runs each command N times, drops the first run as warm-up, reports best/median/mean (ms).
"""
import subprocess, time, statistics, json, sys, os, shutil, hashlib  # nosec B404 - benchmark harness needs subprocess to time the linehash binary

ITERS = 7   # actual measured runs (after 1 warm-up)
WARMUP = 1

def time_cmd(cmd, stdin_data=None, env=None):
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, input=stdin_data, env=env)  # nosec B603 - cmd is a static, hard-coded benchmark argv list
    t1 = time.perf_counter()
    return (t1 - t0) * 1000.0, r.returncode, len(r.stdout)

def bench(label, cmd, stdin_data=None):
    times = []
    bytes_out = 0
    rc = 0
    for _ in range(WARMUP):
        time_cmd(cmd, stdin_data)
    for _ in range(ITERS):
        ms, rc, bo = time_cmd(cmd, stdin_data)
        times.append(ms)
        bytes_out = bo
    return {
        "label": label,
        "cmd": " ".join(cmd) if isinstance(cmd, list) else cmd,
        "best_ms": round(min(times), 3),
        "median_ms": round(statistics.median(times), 3),
        "mean_ms": round(statistics.mean(times), 3),
        "stdev_ms": round(statistics.stdev(times) if len(times) > 1 else 0, 3),
        "iters": ITERS,
        "rc": rc,
        "stdout_bytes": bytes_out,
    }

def fmt_row(r):
    return f"{r['label']:<48s}  best={r['best_ms']:>9.3f}  med={r['median_ms']:>9.3f}  mean={r['mean_ms']:>9.3f}  stdev={r['stdev_ms']:>8.3f}  bytes={r['stdout_bytes']}"

def section(title):
    print(f"\n=== {title} ===")

results = []

def add(r):
    results.append(r)
    print(fmt_row(r))

LH = shutil.which("linehash")
RG = shutil.which("rg")
SED = shutil.which("sed")
GREP = shutil.which("grep")
AWK = shutil.which("awk")

assert LH, "linehash not on PATH"  # nosec B101 - sanity check; harness is not run under python -O

sizes = ["1k", "10k", "100k"]
files = {s: f"/tmp/lh-bench/file_{s}.txt" for s in sizes}  # nosec B108 - explicit benchmark sandbox path, not a security boundary

# Snapshot original files (we'll restore between mutating tests)
def _read_text(p):
    with open(p, "r", encoding="utf-8") as fh:
        return fh.read()


originals = {s: _read_text(files[s]) for s in sizes}

def restore(s):
    with open(files[s], "w", encoding="utf-8") as fh:
        fh.write(originals[s])

section("READ (full file dump with anchors)")
for s in sizes:
    add(bench(f"linehash read {s}",          [LH, "read", files[s]]))
    add(bench(f"linehash read {s} --json",   [LH, "read", files[s], "--json"]))
    add(bench(f"cat {s} (baseline raw read)",[ "cat", files[s]]))

section("INDEX (just line:hash)")
for s in sizes:
    add(bench(f"linehash index {s}",         [LH, "index", files[s]]))
    add(bench(f"linehash index {s} --json",  [LH, "index", files[s], "--json"]))

section("STATS / DOCTOR")
for s in sizes:
    add(bench(f"linehash stats {s}",         [LH, "stats", files[s]]))
    add(bench(f"linehash doctor {s}",        [LH, "doctor", files[s]]))

section("VERIFY (single anchor)")
for s in sizes:
    # produce one valid anchor (line 1)
    out = subprocess.run([LH, "index", files[s], "--json"], capture_output=True).stdout  # nosec B603
    d = json.loads(out)
    a = d["lines"][0]
    anchor = f"{a['n']}:{a['hash']}"
    add(bench(f"linehash verify {s} (1 anchor)", [LH, "verify", files[s], anchor]))

section("GREP — common term, indexed vs --no-index vs ripgrep vs grep")
PATTERN = "function"
for s in sizes:
    add(bench(f"linehash grep {s} '{PATTERN}' (indexed)",      [LH, "grep", files[s], PATTERN]))
    add(bench(f"linehash grep {s} '{PATTERN}' --no-index",     [LH, "grep", files[s], PATTERN, "--no-index"]))
    if RG:
        add(bench(f"rg {s} '{PATTERN}' (no anchors)",          [RG, "-n", PATTERN, files[s]]))
    add(bench(f"grep {s} -n '{PATTERN}' (no anchors)",         ["grep", "-n", PATTERN, files[s]]))

section("GREP — rare regex, indexed vs --no-index vs ripgrep")
PATTERN_RE = "^00009[0-9]{2}: function"
for s in sizes:
    add(bench(f"linehash grep {s} (regex, indexed)",           [LH, "grep", files[s], PATTERN_RE]))
    add(bench(f"linehash grep {s} (regex, --no-index)",        [LH, "grep", files[s], PATTERN_RE, "--no-index"]))
    if RG:
        add(bench(f"rg {s} (regex)",                            [RG, "-n", PATTERN_RE, files[s]]))

section("ANNOTATE — exact-substring match")
for s in sizes:
    add(bench(f"linehash annotate {s} 'function'",             [LH, "annotate", files[s], "function"]))

section("EDIT — single-line replacement at top")
for s in sizes:
    out = subprocess.run([LH, "index", files[s], "--json"], capture_output=True).stdout  # nosec B603
    d = json.loads(out)
    a = d["lines"][0]
    anchor = f"{a['n']}:{a['hash']}"
    new = "MODIFIED LINE FOR BENCH"
    # linehash edit (mutating; restore before each measurement)
    cmd = [LH, "edit", files[s], anchor, new]
    times = []
    for _ in range(WARMUP):
        restore(s)
        time_cmd(cmd)
    for _ in range(ITERS):
        restore(s)
        ms, rc, bo = time_cmd(cmd)
        times.append(ms)
    r = {"label": f"linehash edit {s} (line 1)", "cmd": " ".join(cmd),
         "best_ms": round(min(times),3), "median_ms": round(statistics.median(times),3),
         "mean_ms": round(statistics.mean(times),3),
         "stdev_ms": round(statistics.stdev(times) if len(times)>1 else 0,3),
         "iters": ITERS, "rc": 0, "stdout_bytes": 0}
    add(r); restore(s)

    # sed in-place
    cmd_sed = ["sed", "-i", f"1c\\{new}", files[s]]
    times = []
    for _ in range(WARMUP):
        restore(s); time_cmd(cmd_sed)
    for _ in range(ITERS):
        restore(s); ms, rc, _ = time_cmd(cmd_sed); times.append(ms)
    r = {"label": f"sed -i 1c {s} (baseline)", "cmd": " ".join(cmd_sed),
         "best_ms": round(min(times),3), "median_ms": round(statistics.median(times),3),
         "mean_ms": round(statistics.mean(times),3),
         "stdev_ms": round(statistics.stdev(times) if len(times)>1 else 0,3),
         "iters": ITERS, "rc": 0, "stdout_bytes": 0}
    add(r); restore(s)

section("PATCH — 5 ops in one transaction")
for s in sizes:
    out = subprocess.run([LH, "index", files[s], "--json"], capture_output=True).stdout  # nosec B603
    d = json.loads(out)
    pick = [d["lines"][i] for i in [0, 4, 9, 50 % len(d["lines"]), 99 % len(d["lines"])]]
    ops = [{"op":"edit","anchor":f"{p['n']}:{p['hash']}","content":f"PATCHED {p['n']}"} for p in pick]
    patch = {"ops": ops}
    pf = f"/tmp/lh-bench/patch_{s}.json"  # nosec B108
    with open(pf, "w", encoding="utf-8") as f:
        json.dump(patch, f)
    cmd = [LH, "patch", files[s], pf]
    times = []
    for _ in range(WARMUP):
        restore(s); time_cmd(cmd)
    for _ in range(ITERS):
        restore(s); ms, rc, _ = time_cmd(cmd); times.append(ms)
    r = {"label": f"linehash patch {s} (5 ops)", "cmd": " ".join(cmd),
         "best_ms": round(min(times),3), "median_ms": round(statistics.median(times),3),
         "mean_ms": round(statistics.mean(times),3),
         "stdev_ms": round(statistics.stdev(times) if len(times)>1 else 0,3),
         "iters": ITERS, "rc": 0, "stdout_bytes": 0}
    add(r); restore(s)

section("EXPLODE / IMPLODE round-trip (small only — explode is one file per line)")
out_dir = "/tmp/lh-bench/explode_1k"  # nosec B108
shutil.rmtree(out_dir, ignore_errors=True)
add(bench(f"linehash explode 1k",                  [LH, "explode", files["1k"], "--out", out_dir, "--force"]))
add(bench(f"linehash implode 1k",                  [LH, "implode", out_dir, "--out", "/tmp/lh-bench/imploded_1k.txt"]))  # nosec B108

section("OUTLINE / DEPS / SYMBOL on linehash repo")
RUST_FILE = "/data/projects/linehash/crates/core/document.rs"
add(bench("linehash outline document.rs",          [LH, "outline", RUST_FILE]))
add(bench("linehash deps --file document.rs",      [LH, "deps", "--file", RUST_FILE]))
add(bench("linehash symbol main (broad)",          [LH, "symbol", "main", "--scope", "/data/projects/linehash/crates/core"]))

section("MCP startup + tools/list (cold per-call)")
init = json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}})
listt = json.dumps({"jsonrpc":"2.0","id":2,"method":"tools/list"})
stdin = (init + "\n" + listt + "\n").encode()
add(bench("linehash mcp init+list (timeout-bounded)",
          ["timeout", "3", LH, "mcp"], stdin_data=stdin))

with open("/tmp/lh-bench/results.json", "w", encoding="utf-8") as f:  # nosec B108
    json.dump(results, f, indent=2)
print(f"\nWrote {len(results)} rows to /tmp/lh-bench/results.json")
