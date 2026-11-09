#!/usr/bin/env python3
"""hashline feature benchmark — cross-platform, no shell required.

Writes bench-results/bench-YYYY-MM-DD.md. Run via:

    scripts/bench-features.sh            # wrapper (bash)
    python3 scripts/bench.py             # direct

Env:
    HASHLINE_BIN  binary to bench (default: target/release/hashline[.exe])
    FX_DIR        fixture dir (default: <temp>/hashline-bench)
"""
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = os.environ.get("HASHLINE_BIN")
if not BIN:
    for cand in (ROOT / "target/release/hashline.exe",
                 ROOT / "target/release/hashline"):
        if cand.exists():
            BIN = str(cand)
            break
    else:
        sys.exit("binary not found: run `cargo build --release` first")
FX = Path(os.environ.get("FX_DIR", Path(tempfile.gettempdir()) / "hashline-bench"))
OUT = ROOT / "bench-results" / f"bench-{datetime.now():%Y-%m-%d}.md"
WARMUP, RUNS = 2, 5


def gen_fixtures():
    FX.mkdir(parents=True, exist_ok=True)
    def gen(n, path):
        if path.exists() and path.stat().st_size > 0:
            return
        path.write_text("".join(
            f"pub fn func_{i}(x: i32) -> i32 {{ x + {i} }}\n" for i in range(n)))
    gen(100, FX / "small.rs")
    gen(10_000, FX / "medium.rs")
    gen(100_000, FX / "large.rs")
    gen(500_000, FX / "huge.rs")


def pick_anchor(path, n):
    """Anchor 'N:hh' for file line n (1-based) from `read` output."""
    out = subprocess.check_output([BIN, "read", str(path)], text=True)
    line = out.splitlines()[n]           # line 0 = [path#hash] header
    return line.split("|")[0]


def run_once(argv):
    t0 = time.perf_counter()
    r = subprocess.run(argv, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    return (time.perf_counter() - t0) * 1000, r.returncode


def esc(label):
    return label.replace("|", "\\|")


def bench(label, argv, prepare=None):
    """Time argv; `prepare` callable runs before each timed iteration."""
    print(f"  {label}", file=sys.stderr)
    for _ in range(WARMUP):
        if prepare:
            prepare()
        run_once(argv)
    times = []
    ok = True
    for _ in range(RUNS):
        if prepare:
            prepare()
        ms, rc = run_once(argv)
        times.append(ms)
        ok = ok and rc == 0
    if not ok or not times:
        return f"| {esc(label)} | FAIL | — |"
    return (f"| {esc(label)} | {statistics.mean(times):.1f} ms | "
            f"{min(times):.1f}–{max(times):.1f} ms |")


def main():
    gen_fixtures()

    small = FX / "small.rs"
    medium = FX / "medium.rs"
    large = FX / "large.rs"
    huge = FX / "huge.rs"
    m = FX / "m.rs"

    a_small = pick_anchor(small, 50)
    a_med = pick_anchor(medium, 5000)
    a_large = pick_anchor(large, 50000)
    a_huge = pick_anchor(huge, 250000)

    # 10-op patch fixture
    patch10 = FX / "patch10.txt"
    anchors = [pick_anchor(large, n) for n in
               (1000, 5000, 10000, 20000, 30000, 40000, 50000, 60000, 70000, 80000)]
    patch10.write_text("".join(
        f"SWAP {a}:+patched line {i}\n"
        for i, a in enumerate(anchors, 1)))

    version = subprocess.run([BIN, "--version"], capture_output=True,
                             text=True).stdout.strip() or "unknown"

    lines = []
    add = lines.append
    add(f"# hashline benchmark — {datetime.now():%Y-%m-%d %H:%M}\n")
    add(f"**Binary:** `{BIN}`  \n**Version:** {version}  \n")
    add(f"**Host:** {os.name} {sys.platform}  \n")
    add("**Fixtures:** small=100L, medium=10kL, large=100kL, huge=500kL  \n")
    add("\n---\n\n## Startup\n\n| Command | Mean | Range |\n|---------|-----:|------:|")

    add(bench("hashline --version", [BIN, "--version"]))
    add(bench("hashline guide", [BIN, "guide"]))

    add("\n## Read & orient\n\n| Command | Mean | Range |\n|---------|-----:|------:|")
    for label, f, extra in [
        ("read small.rs (100 L)", small, []),
        ("read medium.rs (10k L)", medium, []),
        ("read large.rs (100k L)", large, []),
        ("read huge.rs (500k L)", huge, []),
        ("read large.rs --json", large, ["--json"]),
        ("read huge.rs --json", huge, ["--json"]),
    ]:
        add(bench(label, [BIN, "read", str(f), *extra]))
    add(bench("find-block large.rs", [BIN, "find-block", str(large), a_large]))

    add("\n## Mutations\n\n> Each mutation rewrites the whole file; the fixture "
        "is restored before every timed run.\n\n| Command | Mean | Range |\n|---------|-----:|------:|")

    def prep(src):
        return lambda: shutil.copyfile(src, m)

    add(bench("patch small.rs SWAP 1 line", [BIN, "patch", str(m),
              f"SWAP {a_small}:+replaced"], prep(small)))
    add(bench("patch medium.rs SWAP 1 line", [BIN, "patch", str(m),
              f"SWAP {a_med}:+replaced"], prep(medium)))
    add(bench("patch large.rs SWAP 1 line", [BIN, "patch", str(m),
              f"SWAP {a_large}:+replaced"], prep(large)))
    add(bench("patch large.rs INS.TAIL", [BIN, "patch", str(m),
              "INS.TAIL:+tail line"], prep(large)))
    add(bench("patch large.rs 10 ops", [BIN, "patch", str(m),
              f"@{patch10}"], prep(large)))
    add(bench("patch huge.rs SWAP 1 line", [BIN, "patch", str(m),
              f"SWAP {a_huge}:+replaced"], prep(huge)))
    add(bench("write file (--force)", [BIN, "write", str(m), "x", "--force"],
              prep(large)))

    add("\n## Dry-run (no write)\n\n| Command | Mean | Range |\n|---------|-----:|------:|")
    add(bench("patch large.rs --dry-run",
              [BIN, "patch", str(large), f"SWAP {a_large}:+replaced", "--dry-run"]))
    add(bench("patch huge.rs --dry-run",
              [BIN, "patch", str(huge), f"SWAP {a_huge}:+replaced", "--dry-run"]))
    add(bench("patch large.rs --dry-run --json",
              [BIN, "patch", str(large), f"SWAP {a_large}:+replaced",
               "--dry-run", "--json"]))

    add("\n## hashline vs sed (str-replace) comparison\n\n"
        "> hashline adds content-hash safety (stale anchor detection, atomic "
        "writes) at comparable raw speed to sed. sed is a pure text-stream "
        "tool with no safety checks. Fixture line 50000.\n\n"
        "### Single-line replace on large.rs (100k L)\n\n"
        "| Tool | Mean | Range |\n|------|-----:|------:|")
    add(bench("hashline patch SWAP | large.rs",
              [BIN, "patch", str(m), f"SWAP {a_large}:+REPLACED_BY_BENCHMARK"],
              prep(large)))
    if shutil.which("sed"):
        add(bench("sed -i line replace | large.rs",
                  ["sed", "-i", "50000s/.*/REPLACED_BY_BENCHMARK/", str(m)],
                  prep(large)))
    else:
        add("| sed -i line replace | large.rs | n/a | — |")

    add("\n## Notes\n\n"
        "- Anchors are content hashes: edits stay valid when unrelated lines "
        "move; stale anchors fail loudly instead of editing the wrong line.\n"
        "- `--safe` (atomic temp+rename) is slower than the default write path "
        "and is not benched here; use it when crash-safety matters more than "
        "throughput.\n")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
