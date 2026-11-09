#!/usr/bin/env bash
# hashline e2e suite — happy cases + edge cases for every command.
#
# Usage:
#   bash scripts/e2e.sh [path-to-hashline-binary]
#   FILTER=ins bash scripts/e2e.sh          # only cases whose name matches
#   bash scripts/e2e.sh --list              # list case names, run nothing
#
# Adding a regression: append a `t "name" case_fn` block at the end and
# write the case as a function. Each case gets a fresh $WORK dir.
#
# Exit: 0 = all passed, 1 = at least one failure.

set -uo pipefail

HASHLINE="${1:-hashline}"
# ${1:-} throughout: bare $1 is unbound under `set -u` when run with no args.
if [ "${1:-}" = "--list" ] || [ "${1:-}" = "-l" ]; then
    LIST_ONLY=1
    HASHLINE="${2:-hashline}"
else
    LIST_ONLY=0
fi
case "$HASHLINE" in --list|-l) HASHLINE="hashline";; esac

PASS=0; FAIL=0; TOTAL=0; SKIP=0
FILTER="${FILTER:-}"
FAILED_NAMES=()
CURRENT=""
ROOT=$(mktemp -d)
WORK="$ROOT"
trap 'rm -rf "$ROOT"' EXIT

# ── Assertions ───────────────────────────────────────────────────────

_fail() {
    FAIL=$((FAIL + 1)); TOTAL=$((TOTAL + 1))
    FAILED_NAMES+=("$CURRENT: $1")
    echo "  ✗ $1"
    [ $# -gt 1 ] && printf '      expected: %s\n      actual:   %s\n' "$2" "$3"
}

_ok() { PASS=$((PASS + 1)); TOTAL=$((TOTAL + 1)); }

assert_eq() {
    if [ "$2" = "$3" ]; then _ok; else _fail "$1" "$2" "$3"; fi
}

assert_ne() {
    if [ "$2" != "$3" ]; then _ok; else _fail "$1" "different from '$2'"; fi
}

assert_contains() {
    if printf '%s' "$2" | grep -qF -- "$3"; then _ok; else _fail "$1" "contains '$3'" "$2"; fi
}

assert_not_contains() {
    if printf '%s' "$2" | grep -qF -- "$3"; then _fail "$1" "does NOT contain '$3'" "$2"; else _ok; fi
}

assert_exit() {
    if [ "$2" = "$3" ]; then _ok; else _fail "$1" "exit $2" "exit $3"; fi
}

# assert_file <label> <path> <expected-content>
# Compares content ignoring trailing newlines — use for "what lines does the
# file hold". Use assert_file_exact when the trailing newline itself matters.
assert_file() {
    local actual expected
    actual=$(printf '%sx' "$(cat "$2" 2>/dev/null)"); actual=${actual%x}
    expected=$(printf '%sx' "$3"); expected=${expected%x}
    actual=${actual%$'\n'}; actual=${actual%$'\n'}
    expected=${expected%$'\n'}; expected=${expected%$'\n'}
    if [ "$expected" = "$actual" ]; then _ok; else _fail "$1" "$(printf '%q' "$expected")" "$(printf '%q' "$actual")"; fi
}

# assert_file_exact <label> <path> <expected-content>  (byte-exact, incl. newline)
assert_file_exact() {
    local actual
    actual=$(printf '%sx' "$(cat "$2" 2>/dev/null)"); actual=${actual%x}
    if [ "$3" = "$actual" ]; then _ok; else _fail "$1" "$(printf '%q' "$3")" "$(printf '%q' "$actual")"; fi
}

# ── Fixtures ─────────────────────────────────────────────────────────

# mk <name> <content> — write into the case's work dir, newline-terminated.
mk() { printf '%s\n' "$2" > "$WORK/$1"; }

# hash_of <file> <line> — current 2-char hash of a line, straight from `read`.
hash_of() {
    "$HASHLINE" read "$WORK/$1" 2>/dev/null \
        | awk -F'[:|]' -v n="$2" '$1==n {gsub(/ /,"",$2); print $2; exit}'
}

# anchor <file> <line> — "line:hash".
anchor() { printf '%s:%s' "$2" "$(hash_of "$1" "$2")"; }

# A hash that is guaranteed not to be the one on that line.
stale_hash_of() {
    local h; h=$(hash_of "$1" "$2")
    if [ "$h" = "00" ]; then echo "01"; else echo "00"; fi
}

# ── Runner ───────────────────────────────────────────────────────────

run_case() {
    local name="$1" fn="$2"
    if [ -n "$FILTER" ] && ! printf '%s' "$name" | grep -qi -- "$FILTER"; then
        SKIP=$((SKIP + 1)); return
    fi
    CURRENT="$name"
    WORK="$ROOT/$(printf '%s' "$name" | tr -cs 'A-Za-z0-9' '_')"
    mkdir -p "$WORK"
    echo "── $name"
    "$fn"
}

t() {
    if [ "$LIST_ONLY" = "1" ]; then echo "$1"; return; fi
    run_case "$1" "$2"
}

# ═════════════════════════════════════════════════════════════════════
# read
# ═════════════════════════════════════════════════════════════════════

case_read_basic() {
    mk f.txt "alpha
beta
gamma"
    OUT=$("$HASHLINE" read "$WORK/f.txt")
    assert_exit "read exits 0" 0 $?
    assert_contains "read has header" "$OUT" "f.txt#"
    assert_contains "read shows line 1" "$OUT" "1:"
    assert_contains "read shows content" "$OUT" "|alpha"
    assert_eq "read line count" "3" "$(printf '%s\n' "$OUT" | grep -c '^[0-9]')"
}

case_read_json() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" read "$WORK/f.txt" --json)
    assert_exit "read --json exits 0" 0 $?
    assert_contains "json has path" "$OUT" '"path":'
    assert_contains "json has lines" "$OUT" '"lines":'
    assert_contains "json content a" "$OUT" '"content":"a"'
    assert_contains "json n=1" "$OUT" '"n":1'
}

case_read_empty_file() {
    : > "$WORK/empty.txt"
    OUT=$("$HASHLINE" read "$WORK/empty.txt" 2>&1)
    assert_exit "read empty exits 0" 0 $?
    assert_contains "read empty has header" "$OUT" "empty.txt#"
}

case_read_missing_file() {
    OUT=$("$HASHLINE" read "$WORK/nope.txt" 2>&1)
    assert_exit "read missing exits 1" 1 $?
    assert_contains "read missing errors" "$OUT" "ERR"
}

case_read_binary_file() {
    printf '\xff\xfe\x00\x01' > "$WORK/bin.dat"
    OUT=$("$HASHLINE" read "$WORK/bin.dat" 2>&1)
    assert_exit "read binary exits 1" 1 $?
    assert_contains "read binary errors" "$OUT" "ERR"
}

case_read_crlf() {
    printf 'a\r\nb\r\n' > "$WORK/crlf.txt"
    OUT=$("$HASHLINE" read "$WORK/crlf.txt" 2>&1)
    assert_exit "read CRLF exits 0" 0 $?
    assert_not_contains "CRLF stripped in anchors" "$OUT" $'\r'
}

case_read_unicode() {
    mk u.txt "café
nguyễn
日本"
    OUT=$("$HASHLINE" read "$WORK/u.txt" 2>&1)
    assert_exit "read unicode exits 0" 0 $?
    assert_contains "unicode preserved" "$OUT" "café"
    assert_contains "cjk preserved" "$OUT" "日本"
}

case_read_no_trailing_newline() {
    printf 'a\nb' > "$WORK/ntn.txt"
    OUT=$("$HASHLINE" read "$WORK/ntn.txt" 2>&1)
    assert_exit "read no-trailing-nl exits 0" 0 $?
    assert_eq "no-trailing-nl line count" "2" "$(printf '%s\n' "$OUT" | grep -c '^[0-9]')"
}

# ═════════════════════════════════════════════════════════════════════
# write / remove / rename
# ═════════════════════════════════════════════════════════════════════

case_write_new() {
    OUT=$("$HASHLINE" write "$WORK/new.txt" "hello" 2>&1)
    assert_exit "write new exits 0" 0 $?
    assert_contains "write new OK" "$OUT" "OK"
    assert_contains "write new reports lines" "$OUT" "lines=1"
    assert_file "write new content" "$WORK/new.txt" "hello"
}

case_write_existing_needs_force() {
    mk exists.txt "old"
    OUT=$("$HASHLINE" write "$WORK/exists.txt" "new" 2>&1)
    assert_exit "write existing exits 1" 1 $?
    assert_contains "write existing errors" "$OUT" "ERR"
    assert_file "write existing untouched" "$WORK/exists.txt" "old"
}

case_write_force() {
    mk exists.txt "old"
    OUT=$("$HASHLINE" write "$WORK/exists.txt" "new" --force 2>&1)
    assert_exit "write --force exits 0" 0 $?
    assert_file "write --force overwrote" "$WORK/exists.txt" "new"
}

case_write_json() {
    OUT=$("$HASHLINE" write "$WORK/j.txt" "hi" --json 2>&1)
    assert_exit "write --json exits 0" 0 $?
    assert_contains "write json success" "$OUT" '"success":true'
}

case_write_safe() {
    OUT=$("$HASHLINE" write "$WORK/s.txt" "atomic" --safe 2>&1)
    assert_exit "write --safe exits 0" 0 $?
    assert_file "write --safe content" "$WORK/s.txt" "atomic"
}

case_remove_file() {
    mk gone.txt "x"
    OUT=$("$HASHLINE" remove "$WORK/gone.txt" 2>&1)
    assert_exit "remove exits 0" 0 $?
    assert_contains "remove OK" "$OUT" "OK"
    if [ -e "$WORK/gone.txt" ]; then _fail "remove deleted the file"; else _ok; fi
}

case_remove_missing() {
    OUT=$("$HASHLINE" remove "$WORK/nope.txt" 2>&1)
    assert_exit "remove missing exits 1" 1 $?
    assert_contains "remove missing errors" "$OUT" "ERR"
}

case_rename_file() {
    mk a.txt "x"
    OUT=$("$HASHLINE" rename "$WORK/a.txt" "$WORK/b.txt" 2>&1)
    assert_exit "rename exits 0" 0 $?
    assert_contains "rename OK" "$OUT" "OK"
    if [ -e "$WORK/a.txt" ]; then _fail "rename removed source"; else _ok; fi
    if [ -e "$WORK/b.txt" ]; then _ok; else _fail "rename created destination"; fi
}

case_rename_dst_exists() {
    mk a.txt "x"; mk b.txt "y"
    OUT=$("$HASHLINE" rename "$WORK/a.txt" "$WORK/b.txt" 2>&1)
    assert_exit "rename onto existing exits 1" 1 $?
    assert_file "rename dst untouched" "$WORK/b.txt" "y"
}

# ═════════════════════════════════════════════════════════════════════
# SWAP
# ═════════════════════════════════════════════════════════════════════

case_swap_single() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 2):
+B" 2>&1)
    assert_exit "SWAP exits 0" 0 $?
    assert_contains "SWAP OK" "$OUT" "OK"
    assert_file "SWAP content" "$WORK/f.txt" "a
B
c"
}

case_swap_multi_line_payload() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 2):
+B1
+B2" 2>&1)
    assert_exit "SWAP multi exits 0" 0 $?
    assert_file "SWAP multi content" "$WORK/f.txt" "a
B1
B2
c"
}

case_swap_range() {
    mk f.txt "1
2
3
4"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 2..=3:
+mid" 2>&1)
    assert_exit "SWAP range exits 0" 0 $?
    assert_file "SWAP range content" "$WORK/f.txt" "1
mid
4"
}

case_swap_empty_payload_deletes() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 2):" 2>&1)
    assert_exit "SWAP empty exits 0" 0 $?
    assert_file "SWAP empty deletes line" "$WORK/f.txt" "a
c"
}

case_swap_stale_hash() {
    mk f.txt "a
b
c"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 2:$(stale_hash_of f.txt 2):
+X" 2>&1)
    assert_exit "SWAP stale exits 1" 1 $?
    assert_contains "SWAP stale error" "$OUT" "STALE"
    assert_contains "SWAP stale shows expected" "$OUT" "expected="
    assert_contains "SWAP stale shows actual" "$OUT" "actual="
    assert_file "SWAP stale leaves file untouched" "$WORK/f.txt" "$orig"
}

case_swap_no_hash_accepted() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 1:
+X" 2>&1)
    assert_exit "hashless SWAP exits 0" 0 $?
    assert_file "hashless SWAP content" "$WORK/f.txt" "X
b"
}

case_swap_invalid_anchor() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 99:
+X" 2>&1)
    assert_exit "SWAP out-of-range exits 1" 1 $?
    assert_contains "SWAP out-of-range errors" "$OUT" "ERR"
}

# ═════════════════════════════════════════════════════════════════════
# DEL
# ═════════════════════════════════════════════════════════════════════

case_del_single() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "DEL $(anchor f.txt 2)" 2>&1)
    assert_exit "DEL exits 0" 0 $?
    assert_file "DEL content" "$WORK/f.txt" "a
c"
}

case_del_range() {
    mk f.txt "1
2
3
4"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "DEL 2..=3" 2>&1)
    assert_exit "DEL range exits 0" 0 $?
    assert_file "DEL range content" "$WORK/f.txt" "1
4"
}

case_del_stale_hash() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "DEL 2:$(stale_hash_of f.txt 2)" 2>&1)
    assert_exit "DEL stale exits 1" 1 $?
    assert_contains "DEL stale error" "$OUT" "STALE"
}

case_del_beyond_end() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "DEL 1..=99" 2>&1)
    assert_exit "DEL beyond end exits 1" 1 $?
    assert_contains "DEL beyond end errors" "$OUT" "ERR"
    assert_file "DEL beyond end untouched" "$WORK/f.txt" "a
b"
}

# ═════════════════════════════════════════════════════════════════════
# INS.* — the regression surface for PR #123
# ═════════════════════════════════════════════════════════════════════

case_ins_pre_valid_hash() {
    mk f.txt "alpha
beta
gamma"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.PRE $(anchor f.txt 2):
+added" 2>&1)
    assert_exit "INS.PRE valid exits 0" 0 $?
    assert_file "INS.PRE lands before anchor" "$WORK/f.txt" "alpha
added
beta
gamma"
}

case_ins_post_valid_hash() {
    mk f.txt "alpha
beta
gamma"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST $(anchor f.txt 2):
+added" 2>&1)
    assert_exit "INS.POST valid exits 0" 0 $?
    assert_file "INS.POST lands after anchor" "$WORK/f.txt" "alpha
beta
added
gamma"
}

case_ins_pre_stale_hash_rejected() {
    mk f.txt "alpha
beta
gamma"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.PRE 2:$(stale_hash_of f.txt 2):
+added" 2>&1)
    assert_exit "INS.PRE stale exits 1" 1 $?
    assert_contains "INS.PRE stale error" "$OUT" "STALE"
    assert_contains "INS.PRE stale actual" "$OUT" "actual="
    assert_file "INS.PRE stale leaves file untouched" "$WORK/f.txt" "$orig"
}

case_ins_post_stale_hash_rejected() {
    mk f.txt "alpha
beta
gamma"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST 2:$(stale_hash_of f.txt 2):
+added" 2>&1)
    assert_exit "INS.POST stale exits 1" 1 $?
    assert_contains "INS.POST stale error" "$OUT" "STALE"
    assert_file "INS.POST stale leaves file untouched" "$WORK/f.txt" "$orig"
}

case_ins_hashless_accepted() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.PRE 2:
+p
INS.POST 2:
+q" 2>&1)
    assert_exit "hashless INS exits 0" 0 $?
    assert_file "hashless INS content" "$WORK/f.txt" "a
p
b
q"
}

case_ins_shorthand_ins() {
    # `INS N:` is shorthand for INS.POST and must honor its hash too.
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS 2:$(stale_hash_of f.txt 2):
+x" 2>&1)
    assert_exit "INS shorthand stale exits 1" 1 $?
    assert_contains "INS shorthand stale error" "$OUT" "STALE"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS $(anchor f.txt 2):
+x" 2>&1)
    assert_exit "INS shorthand valid exits 0" 0 $?
    assert_file "INS shorthand content" "$WORK/f.txt" "a
b
x"
}

case_ins_validated_against_original_snapshot() {
    # A SWAP earlier in the batch rewrites the anchor line. The INS hash must
    # still be checked against the ORIGINAL snapshot line, not the live one.
    mk f.txt "alpha
beta
gamma"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 2:
+CHANGED
INS.PRE $(anchor f.txt 2):
+before" 2>&1)
    assert_exit "INS after SWAP, original hash exits 0" 0 $?
    assert_file "INS after SWAP content" "$WORK/f.txt" "alpha
before
CHANGED
gamma"
}

case_ins_stale_after_earlier_shift() {
    # Earlier inserts shift the anchor's live position; hash still checked
    # against the original entry.
    mk f.txt "alpha
beta
gamma"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.HEAD:
+header
INS.POST 2:$(stale_hash_of f.txt 2):
+x" 2>&1)
    assert_exit "INS stale after shift exits 1" 1 $?
    assert_contains "INS stale after shift error" "$OUT" "STALE"
    assert_file "INS stale after shift untouched" "$WORK/f.txt" "$orig"
}

case_ins_head_tail() {
    mk f.txt "x"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.HEAD:
+head
INS.TAIL:
+tail" 2>&1)
    assert_exit "INS.HEAD/TAIL exits 0" 0 $?
    assert_file "INS.HEAD/TAIL content" "$WORK/f.txt" "head
x
tail"
}

case_ins_repeated_same_anchor_stacks() {
    mk f.txt "a"
    local a; a=$(anchor f.txt 1)
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST $a:
+one
INS.POST $a:
+two" 2>&1)
    assert_exit "repeated INS.POST exits 0" 0 $?
    assert_file "repeated INS.POST order" "$WORK/f.txt" "a
one
two"
}

case_ins_beyond_visible_lines() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST 99:
+x" 2>&1)
    assert_exit "INS beyond EOF exits 1" 1 $?
    assert_contains "INS beyond EOF errors" "$OUT" "ERR"
}

case_ins_symbol_only_line_hash() {
    # Symbol-only lines are position-seeded; their hash must still validate
    # and a neighbouring symbol line's hash must be rejected.
    printf 'fn a() {\n}\nfn b() {\n}\n' > "$WORK/s.rs"
    local h2 h4
    h2=$(hash_of s.rs 2); h4=$(hash_of s.rs 4)
    OUT=$("$HASHLINE" patch "$WORK/s.rs" "INS.PRE 2:$h2:
+    // note" 2>&1)
    assert_exit "INS on symbol line, own hash exits 0" 0 $?
    OUT=$("$HASHLINE" patch "$WORK/s.rs" "INS.POST 2:$h4:
+x" 2>&1)
    assert_exit "INS on symbol line, other hash exits 1" 1 $?
    assert_contains "INS symbol mismatch error" "$OUT" "STALE"
}

# ═════════════════════════════════════════════════════════════════════
# Block ops
# ═════════════════════════════════════════════════════════════════════

case_swap_blk() {
    printf 'fn a() {\n    let x = 1;\n}\nfn b() {\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" patch "$WORK/f.rs" "SWAP.BLK $(anchor f.rs 1):
+fn c() {
+}" 2>&1)
    assert_exit "SWAP.BLK exits 0" 0 $?
    assert_contains "SWAP.BLK OK" "$OUT" "OK"
    assert_contains "SWAP.BLK replaced" "$(cat "$WORK/f.rs")" "fn c() {"
}

case_del_blk() {
    printf 'fn a() {\n    let x = 1;\n}\nfn b() {\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" patch "$WORK/f.rs" "DEL.BLK 1" 2>&1)
    assert_exit "DEL.BLK exits 0" 0 $?
    assert_not_contains "DEL.BLK removed block" "$(cat "$WORK/f.rs")" "let x = 1;"
    assert_contains "DEL.BLK kept sibling" "$(cat "$WORK/f.rs")" "fn b()"
}

case_ins_blk_post_pre() {
    printf 'fn a() {\n    let x = 1;\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" patch "$WORK/f.rs" "INS.BLK.POST $(anchor f.rs 1):
+fn after() {
+}" 2>&1)
    assert_exit "INS.BLK.POST exits 0" 0 $?
    assert_contains "INS.BLK.POST inserted" "$(cat "$WORK/f.rs")" "fn after()"
    printf 'fn a() {\n    let x = 1;\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" patch "$WORK/f.rs" "INS.BLK.PRE $(anchor f.rs 1):
+fn before() {
+}" 2>&1)
    assert_exit "INS.BLK.PRE exits 0" 0 $?
    assert_eq "INS.BLK.PRE is first line" "fn before() {" "$(head -1 "$WORK/f.rs")"
}

case_block_stale_hash() {
    printf 'fn a() {\n}\n' > "$WORK/f.rs"
    local orig; orig=$(cat "$WORK/f.rs"; printf x); orig=${orig%x}
    local stale; stale=$(stale_hash_of f.rs 1)
    local -a patches=(
        "SWAP.BLK 1:$stale:
+x"
        "DEL.BLK 1:$stale"
        "INS.BLK.POST 1:$stale:
+x"
        "INS.BLK.PRE 1:$stale:
+x"
    )
    local i=0
    for patch in "${patches[@]}"; do
        i=$((i + 1))
        OUT=$("$HASHLINE" patch "$WORK/f.rs" "$patch" 2>&1)
        assert_exit "block op $i stale exits 1" 1 $?
        assert_contains "block op $i stale errors" "$OUT" "STALE"
        assert_file "block op $i stale untouched" "$WORK/f.rs" "$orig"
    done
}

case_block_degraded_insert_hash() {
    # Unresolvable block degrades to a plain insert on a .txt file, and must
    # still validate the supplied hash.
    mk f.txt "line one
line two"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.BLK.POST 2:$(stale_hash_of f.txt 2):
+x" 2>&1)
    assert_exit "degraded INS.BLK.POST stale exits 1" 1 $?
    assert_contains "degraded block stale error" "$OUT" "STALE"
}

# ═════════════════════════════════════════════════════════════════════
# CUT / PUT
# ═════════════════════════════════════════════════════════════════════

case_cut_put_roundtrip() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "CUT 2 @r
PUT @r <1" 2>&1)
    assert_exit "CUT/PUT exits 0" 0 $?
    assert_file "CUT/PUT moved line" "$WORK/f.txt" "b
a
c"
}

case_cut_stale_hash() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "CUT 2:$(stale_hash_of f.txt 2) @r" 2>&1)
    assert_exit "CUT stale exits 1" 1 $?
    assert_contains "CUT stale errors" "$OUT" "STALE"
}

case_put_missing_register() {
    mk f.txt "a"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "PUT @nope <1" 2>&1)
    assert_exit "PUT missing register exits 1" 1 $?
    assert_contains "PUT missing register errors" "$OUT" "ERR"
}

# ═════════════════════════════════════════════════════════════════════
# Output modes
# ═════════════════════════════════════════════════════════════════════

case_output_compact() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 1):
+X" 2>&1)
    assert_contains "compact has OK header" "$OUT" "OK"
    assert_contains "compact shows modified marker" "$OUT" "~1:"
}

case_output_json() {
    mk f.txt "a"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 1):
+X" --json 2>&1)
    assert_contains "json success" "$OUT" '"success":true'
    assert_contains "json changed type" "$OUT" '"type":"modified"'
}

case_output_verbose() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 1):
+X" --verbose 2>&1)
    assert_contains "verbose shows file header" "$OUT" "f.txt#"
    assert_contains "verbose shows all lines" "$OUT" "|b"
}

case_output_dry_run() {
    mk f.txt "a
b"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 1):
+X" --dry-run 2>&1)
    assert_exit "dry-run exits 0" 0 $?
    assert_contains "dry-run shows diff" "$OUT" "+X"
    assert_file "dry-run leaves file untouched" "$WORK/f.txt" "$orig"
}

case_output_emit_anchors() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $(anchor f.txt 1):
+X" --emit-anchors 2>&1)
    assert_contains "emit-anchors has file header" "$OUT" "f.txt#"
    assert_contains "emit-anchors has all lines" "$OUT" "|b"
}

# ═════════════════════════════════════════════════════════════════════
# Patch semantics / atomicity
# ═════════════════════════════════════════════════════════════════════

case_atomic_batch_on_stale() {
    # A valid op followed by a stale-anchored op must write nothing.
    mk f.txt "a
b
c"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP 1:
+A
INS.PRE 2:$(stale_hash_of f.txt 2):
+x" 2>&1)
    assert_exit "batch with stale exits 1" 1 $?
    assert_contains "batch with stale errors" "$OUT" "STALE"
    assert_file "batch with stale is atomic" "$WORK/f.txt" "$orig"
}

case_patch_stdin_multi_op() {
    mk f.txt "a
b
c"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" - <<'EOF'
SWAP 1:
+A
DEL 3
INS.TAIL:
+tail
EOF
)
    assert_exit "stdin multi-op exits 0" 0 $?
    assert_file "stdin multi-op content" "$WORK/f.txt" "A
b
tail"
}

case_patch_stale_after_external_change() {
    mk f.txt "orig1
orig2
orig3"
    local a; a=$(anchor f.txt 2)
    printf 'orig1\nEXTERNAL\norig3\n' > "$WORK/f.txt"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "SWAP $a:
+X" 2>&1)
    assert_exit "stale after external change exits 1" 1 $?
    assert_contains "stale after external change errors" "$OUT" "STALE"
    assert_file "external change preserved" "$WORK/f.txt" "orig1
EXTERNAL
orig3"
}

case_patch_missing_file() {
    OUT=$("$HASHLINE" patch "$WORK/nope.txt" "INS.TAIL:
+x" 2>&1)
    assert_exit "patch missing file exits 1" 1 $?
    assert_contains "patch missing file errors" "$OUT" "ERR"
}

case_patch_unknown_op_rejected() {
    mk f.txt "a"
    local orig; orig=$(cat "$WORK/f.txt"; printf x); orig=${orig%x}
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "FOOBAR 1:
+x" 2>&1)
    assert_exit "unknown op exits 1" 1 $?
    assert_file "unknown op untouched" "$WORK/f.txt" "$orig"
}

case_patch_empty_rejected() {
    mk f.txt "a"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "" 2>&1)
    assert_exit "empty patch exits 1" 1 $?
    assert_contains "empty patch errors" "$OUT" "ERR"
}

case_patch_hash_shift_survives_insert() {
    # A later INS targeting a line whose live position shifted must still
    # resolve via the original snapshot hash.
    mk f.txt "a
b
c"
    local b; b=$(anchor f.txt 2)
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.TAIL:
+z
INS.PRE $b:
+before-b" 2>&1)
    assert_exit "INS after tail insert exits 0" 0 $?
    assert_file "INS after tail insert content" "$WORK/f.txt" "a
before-b
b
c
z"
}

# ═════════════════════════════════════════════════════════════════════
# Payload / formatting edge cases
# ═════════════════════════════════════════════════════════════════════

case_payload_dash_item() {
    mk f.txt "a
b"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST $(anchor f.txt 1):
+- item" 2>&1)
    assert_exit "dash payload exits 0" 0 $?
    assert_file "dash payload content" "$WORK/f.txt" "a
- item
b"
}

case_payload_literal_plus() {
    mk f.txt "a"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.POST $(anchor f.txt 1):
++plus" 2>&1)
    assert_exit "escaped plus exits 0" 0 $?
    assert_file "escaped plus content" "$WORK/f.txt" "a
+plus"
}

case_payload_unicode() {
    mk f.txt "a"
    OUT=$("$HASHLINE" patch "$WORK/f.txt" "INS.TAIL:
+café 日本" 2>&1)
    assert_exit "unicode payload exits 0" 0 $?
    assert_contains "unicode payload content" "$(cat "$WORK/f.txt")" "café 日本"
}

case_patch_crlf_preserved() {
    printf 'a\r\nb\r\n' > "$WORK/crlf.txt"
    local h; h=$(hash_of crlf.txt 2)
    OUT=$("$HASHLINE" patch "$WORK/crlf.txt" "SWAP 2:$h:
+B" 2>&1)
    assert_exit "CRLF patch exits 0" 0 $?
    if grep -q $'\r' "$WORK/crlf.txt"; then _ok; else _fail "CRLF preserved after patch"; fi
}

case_patch_no_trailing_newline_preserved() {
    printf 'a\nb' > "$WORK/ntn.txt"
    local h; h=$(hash_of ntn.txt 2)
    OUT=$("$HASHLINE" patch "$WORK/ntn.txt" "SWAP 2:$h:
+B" 2>&1)
    assert_exit "no-trailing-nl patch exits 0" 0 $?
    assert_file_exact "no trailing newline kept" "$WORK/ntn.txt" "a
B"
}

# ═════════════════════════════════════════════════════════════════════
# find-block / guide
# ═════════════════════════════════════════════════════════════════════

case_find_block() {
    printf 'fn a() {\n    x\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" find-block "$WORK/f.rs" 2 2>&1)
    assert_exit "find-block exits 0" 0 $?
    assert_contains "find-block OK" "$OUT" "OK"
    assert_contains "find-block shows opening" "$OUT" "fn a() {"
}

case_find_block_json() {
    printf 'fn a() {\n    x\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" find-block "$WORK/f.rs" 2 --json 2>&1)
    assert_contains "find-block json file" "$OUT" '"file":'
    assert_contains "find-block json lines" "$OUT" '"block_lines":'
}

case_find_block_bad_anchor() {
    printf 'fn a() {\n}\n' > "$WORK/f.rs"
    OUT=$("$HASHLINE" find-block "$WORK/f.rs" 99 2>&1)
    assert_exit "find-block bad anchor exits 1" 1 $?
    assert_contains "find-block bad anchor errors" "$OUT" "ERR"
}

case_guide_runs() {
    OUT=$("$HASHLINE" guide 2>&1)
    assert_exit "guide exits 0" 0 $?
    assert_ne "guide produces output" "" "$OUT"
}

# ═════════════════════════════════════════════════════════════════════
# MCP stdio
# ═════════════════════════════════════════════════════════════════════

mcp_call() {
    local payload="$1"
    printf 'Content-Length: %d\r\n\r\n%s' "${#payload}" "$payload" | "$HASHLINE" mcp 2>/dev/null
}

case_mcp_ins_stale() {
    mk f.txt "alpha
beta
gamma"
    local stale; stale=$(stale_hash_of f.txt 2)
    local op req
    for op in INS.PRE INS.POST; do
        req="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"patch\",\"arguments\":{\"file\":\"$WORK/f.txt\",\"patch\":\"$op 2:$stale:\\n+added\"}}}"
        OUT=$(mcp_call "$req")
        assert_contains "MCP $op stale isError" "$OUT" '"isError":true'
        assert_contains "MCP $op stale message" "$OUT" "changed since last read"
    done
}

case_mcp_ins_valid() {
    mk f.txt "alpha
beta
gamma"
    local req
    req="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"patch\",\"arguments\":{\"file\":\"$WORK/f.txt\",\"patch\":\"INS.PRE $(anchor f.txt 2):\\n+added\"}}}"
    OUT=$(mcp_call "$req")
    assert_not_contains "MCP valid INS has no error" "$OUT" '"isError":true'
    assert_contains "MCP valid INS wrote file" "$(cat "$WORK/f.txt")" "added"
}

case_mcp_read() {
    mk f.txt "a
b"
    local req="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"read\",\"arguments\":{\"file\":\"$WORK/f.txt\"}}}"
    OUT=$(mcp_call "$req")
    assert_contains "MCP read content" "$OUT" "a"
    assert_not_contains "MCP read no error" "$OUT" '"isError":true'
}

# ═════════════════════════════════════════════════════════════════════
# Registry
# ═════════════════════════════════════════════════════════════════════

t "read: basic anchors"                  case_read_basic
t "read: json"                           case_read_json
t "read: empty file"                     case_read_empty_file
t "read: missing file"                   case_read_missing_file
t "read: binary file"                    case_read_binary_file
t "read: crlf"                           case_read_crlf
t "read: unicode"                        case_read_unicode
t "read: no trailing newline"            case_read_no_trailing_newline

t "write: new file"                      case_write_new
t "write: existing needs --force"        case_write_existing_needs_force
t "write: --force"                       case_write_force
t "write: json"                          case_write_json
t "write: --safe"                        case_write_safe

t "remove: file"                         case_remove_file
t "remove: missing"                      case_remove_missing

t "rename: file"                         case_rename_file
t "rename: dst exists"                   case_rename_dst_exists

t "swap: single line"                    case_swap_single
t "swap: multi-line payload"             case_swap_multi_line_payload
t "swap: range"                          case_swap_range
t "swap: empty payload deletes"          case_swap_empty_payload_deletes
t "swap: stale hash"                     case_swap_stale_hash
t "swap: hashless accepted"              case_swap_no_hash_accepted
t "swap: invalid anchor"                 case_swap_invalid_anchor

t "del: single line"                     case_del_single
t "del: range"                           case_del_range
t "del: stale hash"                      case_del_stale_hash
t "del: beyond end"                      case_del_beyond_end

t "ins: INS.PRE valid hash"              case_ins_pre_valid_hash
t "ins: INS.POST valid hash"             case_ins_post_valid_hash
t "ins: INS.PRE stale rejected"          case_ins_pre_stale_hash_rejected
t "ins: INS.POST stale rejected"         case_ins_post_stale_hash_rejected
t "ins: hashless accepted"               case_ins_hashless_accepted
t "ins: INS shorthand validated"         case_ins_shorthand_ins
t "ins: validated vs original snapshot"  case_ins_validated_against_original_snapshot
t "ins: stale after earlier shift"       case_ins_stale_after_earlier_shift
t "ins: HEAD/TAIL"                       case_ins_head_tail
t "ins: repeated anchor stacks"          case_ins_repeated_same_anchor_stacks
t "ins: beyond visible lines"            case_ins_beyond_visible_lines
t "ins: symbol-only line hash"           case_ins_symbol_only_line_hash

t "block: SWAP.BLK"                      case_swap_blk
t "block: DEL.BLK"                       case_del_blk
t "block: INS.BLK.POST/PRE"              case_ins_blk_post_pre
t "block: stale hash on all ops"         case_block_stale_hash
t "block: degraded insert hash"          case_block_degraded_insert_hash

t "cut/put: roundtrip"                   case_cut_put_roundtrip
t "cut: stale hash"                      case_cut_stale_hash
t "put: missing register"                case_put_missing_register

t "output: compact"                      case_output_compact
t "output: json"                         case_output_json
t "output: verbose"                      case_output_verbose
t "output: dry-run"                      case_output_dry_run
t "output: emit-anchors"                 case_output_emit_anchors

t "patch: atomic batch on stale"         case_atomic_batch_on_stale
t "patch: stdin multi-op"                case_patch_stdin_multi_op
t "patch: stale after external change"   case_patch_stale_after_external_change
t "patch: missing file"                  case_patch_missing_file
t "patch: unknown op rejected"           case_patch_unknown_op_rejected
t "patch: empty rejected"                case_patch_empty_rejected
t "patch: hash shift survives insert"    case_patch_hash_shift_survives_insert

t "payload: dash item"                   case_payload_dash_item
t "payload: literal plus"                case_payload_literal_plus
t "payload: unicode"                     case_payload_unicode
t "patch: crlf preserved"                case_patch_crlf_preserved
t "patch: no trailing newline kept"      case_patch_no_trailing_newline_preserved

t "find-block: basic"                    case_find_block
t "find-block: json"                     case_find_block_json
t "find-block: bad anchor"               case_find_block_bad_anchor
t "guide: runs"                          case_guide_runs

t "mcp: INS stale rejected"              case_mcp_ins_stale
t "mcp: INS valid"                       case_mcp_ins_valid
t "mcp: read"                            case_mcp_read

# ═════════════════════════════════════════════════════════════════════

if [ "$LIST_ONLY" = "1" ]; then exit 0; fi

echo ""
echo "========================================="
echo "  $PASS passed, $FAIL failed, $SKIP skipped ($TOTAL assertions)"
echo "========================================="
if [ "$FAIL" -gt 0 ]; then
    echo "  Failures:"
    for n in "${FAILED_NAMES[@]}"; do echo "    - $n"; done
    exit 1
fi
echo "  All passed."
