#!/usr/bin/env bash
# hashline e2e test suite — happy cases, edge cases, and regression tests
# Covers issues #66–#107 and all patch operations.
# Usage: bash e2e-test.sh [path-to-hashline-binary]

set -euo pipefail

HASHLINE="${1:-hashline}"
TMPDIR=$(mktemp -d)
PASS=0
FAIL=0
TOTAL=0

cleanup() { rm -rf "$TMPDIR"; }
trap cleanup EXIT

# ── Helpers ──────────────────────────────────────────────────────────

fail() {
    FAIL=$((FAIL + 1))
    TOTAL=$((TOTAL + 1))
    echo "  FAIL: $1"
    [ -n "${2:-}" ] && echo "    expected: $2" && echo "    got:      $3"
}

pass() {
    PASS=$((PASS + 1))
    TOTAL=$((TOTAL + 1))
}

assert_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass
    else
        fail "$label" "$expected" "$actual"
    fi
}

assert_contains() {
    local label="$1" haystack="$2" needle="$3"
    if echo "$haystack" | grep -qF "$needle"; then
        pass
    else
        fail "$label" "output to contain '$needle'" "$haystack"
    fi
}

assert_not_contains() {
    local label="$1" haystack="$2" needle="$3"
    if echo "$haystack" | grep -qF "$needle"; then
        fail "$label" "output NOT to contain '$needle'" "$haystack"
    else
        pass
    fi
}

assert_exit_code() {
    local label="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass
    else
        fail "$label" "exit code $expected" "exit code $actual"
    fi
}

create_file() {
    local path="$1" content="$2"
    printf '%s\n' "$content" > "$TMPDIR/$path"
}

# Extract full anchor (line:hash) from hashline read output for a given content pattern.
# Usage: get_anchor "$READ_OUTPUT" "Line B" → returns "2:da"
get_anchor() {
    local line_num content_hash
    line_num=$(echo "$1" | grep "$2" | head -1 | cut -d: -f1)
    content_hash=$(echo "$1" | grep "$2" | head -1 | cut -d: -f2 | cut -d'|' -f1)
    echo "${line_num}:${content_hash}"
}

# Extract just the 2-char hash (for use with explicit line numbers).
# Usage: get_hash "$READ_OUTPUT" "Line B" → returns "da"
get_hash() {
    echo "$1" | grep "$2" | head -1 | cut -d: -f2 | cut -d'|' -f1
}

# ── 1. Basic Commands ────────────────────────────────────────────────

echo "=== 1. read ==="

create_file "hello.txt" "Hello World
Line two
Line three"

OUT=$($HASHLINE read "$TMPDIR/hello.txt" 2>&1)
assert_contains "read shows header" "$OUT" "["
assert_contains "read shows hash" "$OUT" ":ee|Hello World"
assert_contains "read shows line numbers" "$OUT" "1:"

OUT=$($HASHLINE read "$TMPDIR/hello.txt" --json 2>&1)
assert_contains "read --json has path" "$OUT" '"path"'
assert_contains "read --json has hash" "$OUT" '"hash"'
assert_contains "read --json has lines" "$OUT" '"lines"'

echo "=== 2. write ==="

OUT=$($HASHLINE write "$TMPDIR/new.txt" "First line
Second line" 2>&1)
assert_contains "write compact shows OK" "$OUT" "OK"
assert_contains "write compact shows lines=" "$OUT" "lines=2"

OUT=$($HASHLINE write "$TMPDIR/new.txt" "overwrite" 2>&1 || true)
assert_contains "write without --force errors" "$OUT" "use --force"

OUT=$($HASHLINE write "$TMPDIR/new.txt" "overwritten" --force 2>&1)
assert_contains "write --force overwrites" "$OUT" "OK"

OUT=$($HASHLINE write "$TMPDIR/json.txt" "content" --json 2>&1)
assert_contains "write --json has success" "$OUT" '"success":true'

echo "=== 3. remove ==="

create_file "to_delete.txt" "delete me"
OUT=$($HASHLINE remove "$TMPDIR/to_delete.txt" 2>&1)
assert_contains "remove compact shows OK" "$OUT" "OK"
assert_eq "remove deletes file" "false" "$([ -f "$TMPDIR/to_delete.txt" ] && echo true || echo false)"

OUT=$($HASHLINE remove "$TMPDIR/nonexistent.txt" 2>&1 || true)
assert_contains "remove nonexistent errors" "$OUT" "ERR"

echo "=== 4. rename ==="

create_file "old_name.txt" "rename me"
OUT=$($HASHLINE rename "$TMPDIR/old_name.txt" "$TMPDIR/new_name.txt" 2>&1)
assert_contains "rename compact shows OK" "$OUT" "OK"
assert_contains "rename shows >" "$OUT" ">"
assert_eq "rename moves file" "false" "$([ -f "$TMPDIR/old_name.txt" ] && echo true || echo false)"
assert_eq "rename creates dest" "true" "$([ -f "$TMPDIR/new_name.txt" ] && echo true || echo false)"

echo "=== 5. find-block ==="

create_file "block.js" "function greet(name) {
  if (name) {
    console.log(name);
  }
  return true;
}"
OUT=$($HASHLINE find-block "$TMPDIR/block.js" 2:8e 2>&1)
assert_contains "find-block compact header" "$OUT" "OK file="
assert_contains "find-block shows lang" "$OUT" "lang=JavaScript"
assert_contains "find-block shows block lines" "$OUT" "if (name)"

# ── 2. Patch Operations ─────────────────────────────────────────────

echo "=== 6. SWAP ==="

create_file "swap.txt" "Line A
Line B
Line C"
# Read to get hashes
READ=$($HASHLINE read "$TMPDIR/swap.txt" 2>&1)
HASH_B=$(get_hash "$READ" "Line B")
OUT=$($HASHLINE patch "$TMPDIR/swap.txt" "SWAP 2:${HASH_B}:
+ Line B-REPLACED" 2>&1)
assert_contains "SWAP shows OK" "$OUT" "OK"
assert_contains "SWAP shows changed line" "$OUT" "Line B-REPLACED"
CONTENT=$(cat "$TMPDIR/swap.txt")
assert_contains "SWAP modifies file" "$CONTENT" "Line B-REPLACED"
# Verify original "Line B" (without -REPLACED) is gone
assert_eq "SWAP removes old line" "0" "$(echo "$CONTENT" | grep -c "Line B$")"

echo "=== 7. SWAP range ==="

create_file "swap_range.txt" "Line 1
Line 2
Line 3
Line 4
Line 5"
READ=$($HASHLINE read "$TMPDIR/swap_range.txt" 2>&1)
H2=$(get_hash "$READ" "Line 2")
H4=$(get_hash "$READ" "Line 4")
OUT=$($HASHLINE patch "$TMPDIR/swap_range.txt" "SWAP 2:${H2}..4:${H4}:
+ REPLACED 2
+ REPLACED 3
+ REPLACED 4" 2>&1)
assert_contains "SWAP range shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/swap_range.txt")
assert_contains "SWAP range replaces" "$CONTENT" "REPLACED 2"
assert_not_contains "SWAP range removes old" "$CONTENT" "Line 2"

echo "=== 8. DEL ==="

create_file "del.txt" "Keep A
Delete B
Keep C"
READ=$($HASHLINE read "$TMPDIR/del.txt" 2>&1)
H2=$(get_hash "$READ" "Delete B")
OUT=$($HASHLINE patch "$TMPDIR/del.txt" "DEL 2:${H2}" 2>&1)
assert_contains "DEL shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/del.txt")
assert_not_contains "DEL removes line" "$CONTENT" "Delete B"
assert_contains "DEL keeps other lines" "$CONTENT" "Keep A"
assert_contains "DEL keeps other lines" "$CONTENT" "Keep C"

echo "=== 9. DEL range ==="

create_file "del_range.txt" "L1
L2
L3
L4
L5"
READ=$($HASHLINE read "$TMPDIR/del_range.txt" 2>&1)
H2=$(get_hash "$READ" "L2")
H4=$(get_hash "$READ" "L4")
OUT=$($HASHLINE patch "$TMPDIR/del_range.txt" "DEL 2:${H2}..4:${H4}" 2>&1)
assert_contains "DEL range shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/del_range.txt")
assert_not_contains "DEL range removes L2" "$CONTENT" "L2"
assert_not_contains "DEL range removes L3" "$CONTENT" "L3"
assert_not_contains "DEL range removes L4" "$CONTENT" "L4"
assert_contains "DEL range keeps L1" "$CONTENT" "L1"
assert_contains "DEL range keeps L5" "$CONTENT" "L5"

echo "=== 10. INS.POST ==="

create_file "ins_post.txt" "Line A
Line B"
READ=$($HASHLINE read "$TMPDIR/ins_post.txt" 2>&1)
H1=$(get_hash "$READ" "Line A")
OUT=$($HASHLINE patch "$TMPDIR/ins_post.txt" "INS.POST 1:${H1}:
+ Inserted after A" 2>&1)
assert_contains "INS.POST shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_post.txt")
assert_contains "INS.POST inserts" "$CONTENT" "Inserted after A"

echo "=== 11. INS.PRE ==="

create_file "ins_pre.txt" "Line A
Line B"
READ=$($HASHLINE read "$TMPDIR/ins_pre.txt" 2>&1)
H2=$(get_hash "$READ" "Line B")
OUT=$($HASHLINE patch "$TMPDIR/ins_pre.txt" "INS.PRE 2:${H2}:
+ Inserted before B" 2>&1)
assert_contains "INS.PRE shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_pre.txt")
assert_contains "INS.PRE inserts" "$CONTENT" "Inserted before B"

echo "=== 12. INS.HEAD ==="

create_file "ins_head.txt" "Existing line"
OUT=$($HASHLINE patch "$TMPDIR/ins_head.txt" "INS.HEAD:
+ First line" 2>&1)
assert_contains "INS.HEAD shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_head.txt")
assert_contains "INS.HEAD inserts at top" "$CONTENT" "First line"

echo "=== 13. INS.TAIL ==="

create_file "ins_tail.txt" "Existing line"
OUT=$($HASHLINE patch "$TMPDIR/ins_tail.txt" "INS.TAIL:
+ Last line" 2>&1)
assert_contains "INS.TAIL shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_tail.txt")
assert_contains "INS.TAIL inserts at end" "$CONTENT" "Last line"

echo "=== 14. SWAP.BLK (Rust) ==="

create_file "swap_blk.rs" "fn hello() {
    let x = 1;
    if true {
        println!(\"ok\");
    }
}"
READ=$($HASHLINE read "$TMPDIR/swap_blk.rs" 2>&1)
H1=$(get_hash "$READ" "fn hello")
OUT=$($HASHLINE patch "$TMPDIR/swap_blk.rs" "SWAP.BLK 1:${H1}:
+fn replaced() {
+    // new body
+}" 2>&1)
assert_contains "SWAP.BLK shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/swap_blk.rs")
assert_contains "SWAP.BLK replaces block" "$CONTENT" "fn replaced()"

echo "=== 15. DEL.BLK ==="

create_file "del_blk.rs" "fn to_delete() {
    let x = 1;
}"
READ=$($HASHLINE read "$TMPDIR/del_blk.rs" 2>&1)
H1=$(get_hash "$READ" "fn to_delete")
OUT=$($HASHLINE patch "$TMPDIR/del_blk.rs" "DEL.BLK 1:${H1}" 2>&1)
assert_contains "DEL.BLK shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/del_blk.rs")
assert_not_contains "DEL.BLK removes block" "$CONTENT" "fn to_delete"

echo "=== 16. INS.BLK.POST ==="

create_file "ins_blk_post.rs" "fn first() {
    let x = 1;
}"
READ=$($HASHLINE read "$TMPDIR/ins_blk_post.rs" 2>&1)
H1=$(get_hash "$READ" "fn first")
OUT=$($HASHLINE patch "$TMPDIR/ins_blk_post.rs" "INS.BLK.POST 1:${H1}:
+fn second() {
+    let y = 2;
+}" 2>&1)
assert_contains "INS.BLK.POST shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_blk_post.rs")
assert_contains "INS.BLK.POST inserts after block" "$CONTENT" "fn second"

echo "=== 17. INS.BLK.PRE ==="

create_file "ins_blk_pre.rs" "fn first() {
    let x = 1;
}"
READ=$($HASHLINE read "$TMPDIR/ins_blk_pre.rs" 2>&1)
H1=$(get_hash "$READ" "fn first")
OUT=$($HASHLINE patch "$TMPDIR/ins_blk_pre.rs" "INS.BLK.PRE 1:${H1}:
+fn zero() {
+    let z = 0;
+}" 2>&1)
assert_contains "INS.BLK.PRE shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/ins_blk_pre.rs")
assert_contains "INS.BLK.PRE inserts before block" "$CONTENT" "fn zero"

echo "=== 18. CUT + PUT ==="

create_file "cut_put.txt" "Alpha
Bravo
Charlie
Delta"
READ=$($HASHLINE read "$TMPDIR/cut_put.txt" 2>&1)
H2=$(get_hash "$READ" "Bravo")
H3=$(get_hash "$READ" "Charlie")
H4=$(get_hash "$READ" "Delta")
OUT=$($HASHLINE patch "$TMPDIR/cut_put.txt" - 2>&1 <<PATCH
CUT 2:${H2}..4:${H4} @moved
PUT @moved <2:${H2}
PATCH
)
assert_contains "CUT+PUT shows OK" "$OUT" "OK"

# ── 3. Multi-Op Patches ─────────────────────────────────────────────

echo "=== 19. Multi-op via stdin ==="

create_file "multi.txt" "Line A
Line B
Line C
Line D
Line E"
READ=$($HASHLINE read "$TMPDIR/multi.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
HD=$(get_hash "$READ" "Line D")
OUT=$($HASHLINE patch "$TMPDIR/multi.txt" - 2>&1 <<PATCH
SWAP 2:${HB}:
+ Line B-NEW
INS.POST 4:${HD}:
+ Line D-INSERTED
PATCH
)
assert_contains "multi-op shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/multi.txt")
assert_contains "multi-op swap applied" "$CONTENT" "Line B-NEW"
assert_contains "multi-op insert applied" "$CONTENT" "Line D-INSERTED"

echo "=== 20. Envelope format ==="

create_file "envelope.txt" "Line 1
Line 2
Line 3"
READ=$($HASHLINE read "$TMPDIR/envelope.txt" 2>&1)
H2=$(get_hash "$READ" "Line 2")
H3=$(get_hash "$READ" "Line 3")
OUT=$($HASHLINE patch "$TMPDIR/envelope.txt" - 2>&1 <<PATCH
*** Begin Patch
SWAP 2:${H2}:
+ Line 2-ENVELOPE
INS.POST 3:${H3}:
+ Line 3-ENVELOPE-INSERT
*** End Patch
PATCH
)
assert_contains "envelope shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/envelope.txt")
assert_contains "envelope swap applied" "$CONTENT" "Line 2-ENVELOPE"

echo "=== 21. Abort marker ==="

create_file "abort.txt" "Keep this
Delete this
Keep too"
READ=$($HASHLINE read "$TMPDIR/abort.txt" 2>&1)
H2=$(get_hash "$READ" "Delete this")
ORIG=$(cat "$TMPDIR/abort.txt")
$HASHLINE patch "$TMPDIR/abort.txt" - 2>&1 <<PATCH || true
*** Begin Patch
SWAP 2:${H2}:
+ SHOULD NOT APPLY
*** Abort
PATCH
CONTENT=$(cat "$TMPDIR/abort.txt")
assert_eq "abort does not modify file" "$ORIG" "$CONTENT"

# ── 4. Edge Cases (from issues) ─────────────────────────────────────

echo "=== 22. Stale anchor detection (#89, #104) ==="

create_file "stale.txt" "Original line 1
Original line 2
Original line 3"
READ=$($HASHLINE read "$TMPDIR/stale.txt" 2>&1)
H2=$(get_hash "$READ" "Original line 2")
# Modify file externally
echo "External change" > "$TMPDIR/stale.txt"
echo "Line 2" >> "$TMPDIR/stale.txt"
echo "Original line 3" >> "$TMPDIR/stale.txt"
OUT=$($HASHLINE patch "$TMPDIR/stale.txt" "SWAP 2:${H2}:
+ Changed" 2>&1 || true)
assert_contains "stale anchor errors" "$OUT" "ERR"
assert_contains "stale anchor shows kind" "$OUT" "STALE"

echo "=== 23. CRLF handling (#101) ==="

printf "Line 1\r\nLine 2\r\nLine 3\r\n" > "$TMPDIR/crlf.txt"
READ=$($HASHLINE read "$TMPDIR/crlf.txt" 2>&1)
H1=$(get_hash "$READ" "Line 1")
OUT=$($HASHLINE patch "$TMPDIR/crlf.txt" "SWAP 1:${H1}:
+ Line 1-REPLACED" --dry-run 2>&1)
assert_not_contains "CRLF dry-run no \\r" "$OUT" "\r"

echo "=== 24. Trailing newline handling (#100, #102) ==="

printf "Line 1\nLine 2\n" > "$TMPDIR/trailing.txt"
READ=$($HASHLINE read "$TMPDIR/trailing.txt" 2>&1)
# Should show 2 lines, not 3
LINE_COUNT=$(echo "$READ" | grep -c "^[0-9]*:")
assert_eq "trailing newline shows correct count" "2" "$LINE_COUNT"

echo "=== 25. Payload escapes (#93) ==="

create_file "escape.txt" "Before
Target line
After"
READ=$($HASHLINE read "$TMPDIR/escape.txt" 2>&1)
HT=$(get_anchor "$READ" "Target line")
OUT=$($HASHLINE patch "$TMPDIR/escape.txt" "SWAP ${HT}:
+ ++Literal plus
+ +-Literal minus" 2>&1)
assert_contains "escape shows OK" "$OUT" "OK"
CONTENT=$(cat "$TMPDIR/escape.txt")
assert_contains "escape ++ produces +" "$CONTENT" "Literal plus"
assert_contains "escape +- produces -" "$CONTENT" "Literal minus"

echo "=== 26. Case-insensitive ops ==="

create_file "case.txt" "Line A
Line B"
READ=$($HASHLINE read "$TMPDIR/case.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
OUT=$($HASHLINE patch "$TMPDIR/case.txt" "swap 2:${HB}:
+ Line B-REPLACED" 2>&1)
assert_contains "case-insensitive shows OK" "$OUT" "OK"

echo "=== 27. A.=B range syntax (#96) ==="

create_file "dot_range.txt" "L1
L2
L3
L4
L5"
READ=$($HASHLINE read "$TMPDIR/dot_range.txt" 2>&1)
H2=$(get_hash "$READ" "L2")
H4=$(get_hash "$READ" "L4")
OUT=$($HASHLINE patch "$TMPDIR/dot_range.txt" "SWAP 2:${H2}.=4:${H4}:
+ X2
+ X3
+ X4" 2>&1)
assert_contains "A.=B range shows OK" "$OUT" "OK"

echo "=== 28. find-block Python (#72) ==="

create_file "py_block.py" "def greet(name):
    if name:
        print(f'Hello {name}')
    return True"
READ=$($HASHLINE read "$TMPDIR/py_block.py" 2>&1)
H2=$(get_hash "$READ" "if name")
OUT=$($HASHLINE find-block "$TMPDIR/py_block.py" 2:${H2} 2>&1)
assert_contains "find-block Python header" "$OUT" "OK"
assert_contains "find-block Python lang" "$OUT" "lang=Python"

echo "=== 29. find-block JS (#98) ==="

create_file "js_block.js" "function hello() {
  if (true) {
    console.log('ok');
  }
}"
READ=$($HASHLINE read "$TMPDIR/js_block.js" 2>&1)
H2=$(get_hash "$READ" "if (true)")
OUT=$($HASHLINE find-block "$TMPDIR/js_block.js" 2:${H2} 2>&1)
assert_contains "find-block JS header" "$OUT" "OK"
assert_contains "find-block JS lang" "$OUT" "lang=JavaScript"

echo "=== 30. rename --force (#99) ==="

create_file "rename_force_a.txt" "content A"
create_file "rename_force_b.txt" "content B"
OUT=$($HASHLINE rename "$TMPDIR/rename_force_a.txt" "$TMPDIR/rename_force_b.txt" --force 2>&1)
assert_contains "rename --force shows OK" "$OUT" "OK"

# ── 5. Output Modes ─────────────────────────────────────────────────

echo "=== 31. --verbose patch ==="

create_file "verbose.txt" "Line A
Line B
Line C"
READ=$($HASHLINE read "$TMPDIR/verbose.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
OUT=$($HASHLINE patch "$TMPDIR/verbose.txt" --verbose "SWAP 2:${HB}:
+ Line B-NEW" 2>&1)
assert_contains "verbose shows header" "$OUT" "["
assert_contains "verbose shows all lines" "$OUT" "Line A"
assert_contains "verbose shows new line" "$OUT" "Line B-NEW"

echo "=== 32. --json patch ==="

create_file "json_patch.txt" "Line A
Line B
Line C"
READ=$($HASHLINE read "$TMPDIR/json_patch.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
OUT=$($HASHLINE patch "$TMPDIR/json_patch.txt" --json "SWAP 2:${HB}:
+ Line B-NEW" 2>&1)
assert_contains "json shows success" "$OUT" '"success":true'
assert_contains "json shows changed" "$OUT" '"changed"'
assert_contains "json shows type modified" "$OUT" '"type":"modified"'

echo "=== 33. --json write ==="

OUT=$($HASHLINE write "$TMPDIR/json_write.txt" "content" --json 2>&1)
assert_contains "json write shows success" "$OUT" '"success":true'
assert_contains "json write shows lines" "$OUT" '"lines"'

echo "=== 34. --json remove ==="

create_file "json_remove.txt" "delete me"
OUT=$($HASHLINE remove "$TMPDIR/json_remove.txt" --json 2>&1)
assert_contains "json remove shows success" "$OUT" '"success":true'

echo "=== 35. --json rename ==="

create_file "json_rename_old.txt" "rename"
OUT=$($HASHLINE rename "$TMPDIR/json_rename_old.txt" "$TMPDIR/json_rename_new.txt" --json 2>&1)
assert_contains "json rename shows success" "$OUT" '"success":true'

echo "=== 36. --json find-block ==="

create_file "json_fb.js" "function test() { return 1; }"
READ=$($HASHLINE read "$TMPDIR/json_fb.js" 2>&1)
H1=$(get_hash "$READ" "function test")
OUT=$($HASHLINE find-block "$TMPDIR/json_fb.js" 1:${H1} --json 2>&1)
assert_contains "json find-block shows file" "$OUT" '"file"'
assert_contains "json find-block shows block_lines" "$OUT" '"block_lines"'

echo "=== 37. --dry-run patch ==="

create_file "dryrun.txt" "Line A
Line B
Line C"
READ=$($HASHLINE read "$TMPDIR/dryrun.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
ORIG=$(cat "$TMPDIR/dryrun.txt")
OUT=$($HASHLINE patch "$TMPDIR/dryrun.txt" "SWAP 2:${HB}:
+ Line B-CHANGED" --dry-run 2>&1)
CONTENT=$(cat "$TMPDIR/dryrun.txt")
assert_eq "dry-run does not modify file" "$ORIG" "$CONTENT"

# ── 6. Error Cases ──────────────────────────────────────────────────

echo "=== 38. Empty patch ==="

create_file "empty_patch.txt" "Line A"
OUT=$($HASHLINE patch "$TMPDIR/empty_patch.txt" "" 2>&1 || true)
assert_contains "empty patch errors" "$OUT" "ERR"

echo "=== 39. Invalid anchor ==="

create_file "invalid_anchor.txt" "Line A"
OUT=$($HASHLINE patch "$TMPDIR/invalid_anchor.txt" "SWAP 99:ff:
+ New" 2>&1 || true)
assert_contains "invalid anchor errors" "$OUT" "ERR"

echo "=== 40. Binary file ==="

printf '\x00\x01\x02\x03' > "$TMPDIR/binary.dat"
OUT=$($HASHLINE read "$TMPDIR/binary.dat" 2>&1 || true)
assert_contains "binary file errors" "$OUT" "ERR"

echo "=== 41. File not found ==="

OUT=$($HASHLINE read "$TMPDIR/nonexistent.txt" 2>&1 || true)
assert_contains "file not found errors" "$OUT" "ERR"

# ── 7. Clipboard Operations ─────────────────────────────────────────

echo "=== 42. PUT without CUT errors ==="

create_file "put_no_cut.txt" "Line A
Line B"
OUT=$($HASHLINE patch "$TMPDIR/put_no_cut.txt" "PUT @nosuch <2" 2>&1 || true)
assert_contains "PUT without CUT errors" "$OUT" "CUT"

echo "=== 43. Anonymous PUT without CUT errors ==="

OUT=$($HASHLINE patch "$TMPDIR/put_no_cut.txt" "PUT <2" 2>&1 || true)
assert_contains "anon PUT without CUT errors" "$OUT" "CUT"

# ── 8. Patch Source Modes ───────────────────────────────────────────

echo "=== 44. Patch via file reference ==="

create_file "patch_ref.txt" "Line A
Line B"
READ=$($HASHLINE read "$TMPDIR/patch_ref.txt" 2>&1)
HB=$(get_hash "$READ" "Line B")
create_file "my.patch" "SWAP 2:${HB}:
+ Line B-FILE"
OUT=$($HASHLINE patch "$TMPDIR/patch_ref.txt" "@$TMPDIR/my.patch" 2>&1)
assert_contains "patch via file shows OK" "$OUT" "OK"

# ── 9. Envelope with file header ────────────────────────────────────

echo "=== 45. Patch with file header ==="

create_file "header_patch.txt" "Line 1
Line 2"
READ=$($HASHLINE read "$TMPDIR/header_patch.txt" 2>&1)
H1=$(get_hash "$READ" "Line 1")
FH=$(echo "$READ" | head -1 | tr -d '[]#' | cut -d: -f2)
OUT=$($HASHLINE patch "$TMPDIR/header_patch.txt" "[header_patch.txt#${FH}]
SWAP 1:${H1}:
+ Line 1-HEADER" 2>&1)
assert_contains "file header patch shows OK" "$OUT" "OK"

# ── 10. Boundary echo detection ─────────────────────────────────────

echo "=== 46. Boundary echo detection ==="

create_file "boundary.txt" "Line A
Line B
Line C"
READ=$($HASHLINE read "$TMPDIR/boundary.txt" 2>&1)
HA=$(get_hash "$READ" "Line A")
HC=$(get_hash "$READ" "Line C")
# SWAP range that echoes boundaries (payload restates unchanged lines)
OUT=$($HASHLINE patch "$TMPDIR/boundary.txt" "SWAP 1:${HA}..3:${HC}:
+ Line A
+ Line B-NEW
+ Line C" 2>&1 || true)
# Should either succeed (with warning) or error — but not corrupt
CONTENT=$(cat "$TMPDIR/boundary.txt")
assert_contains "boundary keeps Line A" "$CONTENT" "Line A"

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "========================================="
echo "  Results: $PASS passed, $FAIL failed, $TOTAL total"
echo "========================================="

if [ "$FAIL" -gt 0 ]; then
    exit 1
else
    echo "  All tests passed!"
    exit 0
fi
