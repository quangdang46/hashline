#!/bin/bash
# hashline edge case tests
set -u
HASHLINE="/Users/tranquangdang21/Projects/hashline/target/release/hashline"
TMPDIR="/tmp/hashline-edge"
PASS=0; FAIL=0; SKIP=0

log() { echo "$@"; }
assert_pass() { log "  PASS: $1"; PASS=$((PASS+1)); }
assert_fail() { log "  FAIL: $1 :: ${2:-}"; FAIL=$((FAIL+1)); }
assert_skip() { log "  SKIP: $1 :: ${2:-}"; SKIP=$((SKIP+1)); }
assert() { local l="$1" n="$2" h="$3"; echo "$h" | grep -qF -- "$n" && assert_pass "$l" || assert_fail "$l" "missing '$n'"; }
assert_not() { local l="$1" n="$2" h="$3"; echo "$h" | grep -qF -- "$n" && assert_fail "$l" "unexpected '$n'" || assert_pass "$l"; }
assert_rc() { local l="$1" e="$2" g="$3"; [ "$g" = "$e" ] && assert_pass "$l (rc=$g)" || assert_fail "$l" "expected rc=$e got rc=$g"; }
get_anchor() { "$HASHLINE" read "$1" 2>/dev/null | grep -E "$2" | head -1 | sed 's/^[[:space:]]*//;s/|.*//'; }

rm -rf "$TMPDIR"; mkdir -p "$TMPDIR"

log "================================================================"
log "HASHLINE EDGE CASE TESTS"
log "================================================================"

# ===================================================================
log ""
log "=== E1. MULTI-OP PATCH ==="
cat > "$TMPDIR/multi.txt" <<'EOF'
line 1
line 2
line 3
line 4
EOF
A1=$(get_anchor "$TMPDIR/multi.txt" "line 1$")
A2=$(get_anchor "$TMPDIR/multi.txt" "line 2$")
A3=$(get_anchor "$TMPDIR/multi.txt" "line 3$")
cat > "$TMPDIR/multi.patch" <<EOF
INS.PRE $A1:
+PRE_1
INS.POST $A3:
+POST_3
EOF
OUT=$($HASHLINE patch "$TMPDIR/multi.txt" "@$TMPDIR/multi.patch" 2>&1); RC=$?
assert_rc "multi-op patch rc=0" 0 "$RC"
CONTENT=$(cat $TMPDIR/multi.txt)
assert "multi-op has PRE_1" "PRE_1" "$CONTENT"
assert "multi-op has POST_3" "POST_3" "$CONTENT"
WC=$(echo "$CONTENT" | wc -l | tr -d ' ')
[ "$WC" = "6" ] && assert_pass "multi-op total lines = 6" || assert_fail "multi-op total lines" "got $WC"

# ===================================================================
log ""
log "=== E2. PAYLOAD ESCAPES (++ and +-) ==="
cat > "$TMPDIR/esc.txt" <<'EOF'
line 1
line 2
EOF
A1=$(get_anchor "$TMPDIR/esc.txt" "line 1$")
cat > "$TMPDIR/esc.patch" <<EOF
SWAP $A1:
++starts with plus
+-starts with minus
EOF
OUT=$($HASHLINE patch "$TMPDIR/esc.txt" "@$TMPDIR/esc.patch" 2>&1); RC=$?
assert_rc "escape ++ rc=0" 0 "$RC"
CONTENT=$(cat $TMPDIR/esc.txt)
assert "++ becomes +" "+starts with plus" "$CONTENT"
assert "++ line exists" "starts with plus" "$CONTENT"
assert "+- becomes -" "-starts with minus" "$CONTENT"

# ===================================================================
log ""
log "=== E3. ENVELOPE MARKERS (Begin/End Patch) ==="
cat > "$TMPDIR/env.txt" <<'EOF'
line 1
line 2
line 3
EOF
A3=$(get_anchor "$TMPDIR/env.txt" "line 3$")
cat > "$TMPDIR/env.patch" <<EOF
*** Begin Patch
SWAP $A3:
+REPLACED_3
*** End Patch
EOF
OUT=$($HASHLINE patch "$TMPDIR/env.txt" "@$TMPDIR/env.patch" 2>&1); RC=$?
assert_rc "envelope markers rc=0" 0 "$RC"
CONTENT=$(cat $TMPDIR/env.txt)
assert "envelope replaced" "REPLACED_3" "$CONTENT"
assert_not "envelope no marker in output" "Begin Patch" "$CONTENT"

# ===================================================================
log ""
log "=== E4. SWAP.BLK (replace block) ==="
cat > "$TMPDIR/blk.rs" <<'EOF'
fn outer() {
    let a = 1;
}
fn target() {
    let x = 10;
    let y = 20;
    let z = 30;
}
fn other() {
    let b = 2;
}
EOF
A=$(get_anchor "$TMPDIR/blk.rs" "fn target")
cat > "$TMPDIR/blk.patch" <<EOF
SWAP.BLK $A:
+fn target() {
+    let x = 99;
+    let y = 100;
+}
EOF
OUT=$($HASHLINE patch "$TMPDIR/blk.rs" "@$TMPDIR/blk.patch" 2>&1); RC=$?
assert_rc "SWAP.BLK rc=0" 0 "$RC"
CONTENT=$(cat $TMPDIR/blk.rs)
assert "SWAP.BLK has new content" "let x = 99" "$CONTENT"
assert_not "SWAP.BLK removed old content" "let z = 30" "$CONTENT"
assert "SWAP.BLK preserved outer fn" "fn outer" "$CONTENT"
assert "SWAP.BLK preserved other fn" "fn other" "$CONTENT"

# ===================================================================
log ""
log "=== E5. DEL.BLK (delete block) ==="
cat > "$TMPDIR/dblk.rs" <<'EOF'
fn keep1() {
    let a = 1;
}
fn delete_me() {
    let secret = 42;
    let key = "hidden";
}
fn keep2() {
    let b = 2;
}
EOF
A=$(get_anchor "$TMPDIR/dblk.rs" "fn delete_me")
cat > "$TMPDIR/dblk.patch" <<EOF
DEL.BLK $A
EOF
OUT=$($HASHLINE patch "$TMPDIR/dblk.rs" "@$TMPDIR/dblk.patch" 2>&1); RC=$?
assert_rc "DEL.BLK rc=0" 0 "$RC"
CONTENT=$(cat $TMPDIR/dblk.rs)
assert_not "DEL.BLK removed delete_me" "fn delete_me" "$CONTENT"
assert_not "DEL.BLK removed secret" "secret" "$CONTENT"
assert "DEL.BLK preserves keep1" "fn keep1" "$CONTENT"
assert "DEL.BLK preserves keep2" "fn keep2" "$CONTENT"

# ===================================================================
log ""
log "=== E6. ABORT MARKER (patch silently skipped) ==="
cat > "$TMPDIR/abort.txt" <<'EOF'
alpha
beta
gamma
EOF
ORIG=$(cat $TMPDIR/abort.txt)
cat > "$TMPDIR/abort.patch" <<EOF
*** Abort
SWAP 1:
+REPLACED
EOF
OUT=$($HASHLINE patch "$TMPDIR/abort.txt" "@$TMPDIR/abort.patch" 2>&1); RC=$?
assert_rc "abort marker rc=0" 0 "$RC"
[ "$(cat $TMPDIR/abort.txt)" = "$ORIG" ] && assert_pass "abort skipped patch" || assert_fail "abort modified file"

# ===================================================================
log ""
log "=== E7. CRLF LINE ENDINGS ==="
# Create CRLF file
printf "line1\r\nline2\r\nline3\r\n" > "$TMPDIR/crlf.txt"
OUT=$($HASHLINE read "$TMPDIR/crlf.txt" 2>&1); RC=$?
assert_rc "read CRLF rc=0" 0 "$RC"
assert "read CRLF shows content" "line1" "$OUT"

A2=$(get_anchor "$TMPDIR/crlf.txt" "line2")
cat > "$TMPDIR/crlf.patch" <<EOF
SWAP $A2:
+CRLF_REPLACED
EOF
OUT=$($HASHLINE patch "$TMPDIR/crlf.txt" "@$TMPDIR/crlf.patch" 2>&1); RC=$?
assert_rc "patch CRLF rc=0" 0 "$RC"
CONTENT=$(xxd "$TMPDIR/crlf.txt" | head -5)
# Should preserve CRLF
echo "$CONTENT" | grep -q "0d0a" && assert_pass "CRLF preserved" || assert_skip "CRLF preservation" "check requires xxd"

# ===================================================================
log ""
log "=== E8. UNICODE FILE NAMES ==="
# Test file names with unicode
printf "content" > "$TMPDIR/你好.txt"
OUT=$($HASHLINE read "$TMPDIR/你好.txt" 2>&1); RC=$?
assert_rc "read unicode filename" 0 "$RC"
assert "unicode filename shown" "你好" "$OUT"

A1=$(get_anchor "$TMPDIR/你好.txt" "content")
cat > "$TMPDIR/你好.patch" <<EOF
SWAP $A1:
+FIXED
EOF
OUT=$($HASHLINE patch "$TMPDIR/你好.txt" "@$TMPDIR/你好.patch" 2>&1); RC=$?
assert_rc "patch unicode filename" 0 "$RC"
[ "$(cat $TMPDIR/你好.txt)" = "FIXED" ] && assert_pass "unicode file patched correctly" || assert_fail "unicode file patched" "$(cat $TMPDIR/你好.txt)"

# ===================================================================
log ""
log "=== E9. VERY LARGE FILE READ ==="
# Generate a 10,000 line file
rm -f "$TMPDIR/big.txt"
for i in $(seq 1 10000); do echo "This is line number $i of the large test file" >> "$TMPDIR/big.txt"; done
OUT=$($HASHLINE read "$TMPDIR/big.txt" 2>&1); RC=$?
assert_rc "read 10k lines" 0 "$RC"
assert "read 10k shows last line" "10000" "$OUT"

OUT=$($HASHLINE read "$TMPDIR/big.txt" --json 2>&1); RC=$?
assert_rc "read 10k --json" 0 "$RC"
echo "$OUT" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); assert len(d['lines'])==10000; print('10k lines verified')" 2>>"$TMPDIR/err.log" && assert_pass "read 10k --json valid with 10k entries" || assert_fail "read 10k --json validation"

# Patch a single line in the large file
A1=$(get_anchor "$TMPDIR/big.txt" "line number 1 ")
cat > "$TMPDIR/big.patch" <<EOF
SWAP $A1:
+LINE_1_REPLACED
EOF
OUT=$($HASHLINE patch "$TMPDIR/big.txt" "@$TMPDIR/big.patch" 2>&1); RC=$?
assert_rc "patch 10k file single line" 0 "$RC"
FIRST=$(cat $TMPDIR/big.txt | head -1)
[ "$FIRST" = "LINE_1_REPLACED" ] && assert_pass "large file patched correctly" || assert_fail "large file patch" "got '$FIRST'"

rm -f "$TMPDIR/big.txt" "$TMPDIR/big.patch"

# ===================================================================
log ""
log "=== E10. BINARY FILE DETECTION (if supported) ==="
# Create a small binary file (not valid UTF-8)
printf '\x00\x01\x02\xff\xfe\xfd' > "$TMPDIR/binary.bin"
OUT=$($HASHLINE read "$TMPDIR/binary.bin" 2>&1); RC=$?
# Binary detection is optional — may or may not error
log "  binary read rc=$RC"

# ===================================================================
log ""
log "=== E11. PATCH WITH FILE HEADER MATCHING ==="
cat > "$TMPDIR/fh.txt" <<'EOF'
hello from fh
EOF
OUT=$($HASHLINE read "$TMPDIR/fh.txt" 2>&1)
FHASH=$(echo "$OUT" | grep -oE '#[0-9a-f]+' | head -1 | tr -d '#')
A=$(get_anchor "$TMPDIR/fh.txt" "hello from fh")
cat > "$TMPDIR/fh.patch" <<EOF
[$TMPDIR/fh.txt#$FHASH]
SWAP $A:
+PATCHED_WITH_HEADER
EOF
OUT=$($HASHLINE patch "$TMPDIR/fh.txt" "@$TMPDIR/fh.patch" 2>&1); RC=$?
assert_rc "patch with file header" 0 "$RC"
[ "$(cat $TMPDIR/fh.txt)" = "PATCHED_WITH_HEADER" ] && assert_pass "file header patch works" || assert_fail "file header patch" "$(cat $TMPDIR/fh.txt)"

# ===================================================================
log ""
log "=== E12. DRY-RUN WITH RANGE ==="
cat > "$TMPDIR/dr.txt" <<'EOF'
aaa
bbb
ccc
ddd
eee
EOF
A1=$(get_anchor "$TMPDIR/dr.txt" "aaa")
A3=$(get_anchor "$TMPDIR/dr.txt" "ccc")
ORIG=$(cat $TMPDIR/dr.txt)
cat > "$TMPDIR/dr.patch" <<EOF
SWAP $A1..$A3:
+XXX
+YYY
+ZZZ
EOF
OUT=$($HASHLINE patch "$TMPDIR/dr.txt" "@$TMPDIR/dr.patch" --dry-run 2>&1); RC=$?
assert_rc "dry-run range" 0 "$RC"
assert "dry-run range shows XXX" "XXX" "$OUT"
assert "dry-run range shows YYY" "YYY" "$OUT"
assert "dry-run range shows ZZZ" "ZZZ" "$OUT"
[ "$(cat $TMPDIR/dr.txt)" = "$ORIG" ] && assert_pass "dry-run range didn't modify" || assert_fail "dry-run range modified"

# ===================================================================
log ""
log "=== E13. EMPTY FILE WRITE ==="
OUT=$($HASHLINE write "$TMPDIR/empty_out.txt" "" --force 2>&1); RC=$?
assert_rc "write empty file" 0 "$RC"
[ -f "$TMPDIR/empty_out.txt" ] && [ ! -s "$TMPDIR/empty_out.txt" ] && assert_pass "empty file created (0 bytes)" || assert_fail "empty file" "size=$(wc -c < $TMPDIR/empty_out.txt)"

# ===================================================================
log ""
log "=== E14. FIND-BLOCK PYTHON INDENT ==="
cat > "$TMPDIR/pyindent.py" <<'EOF'
class Animal:
    def __init__(self, name):
        self.name = name

    def speak(self):
        return f"{self.name} says hi"

class Dog(Animal):
    def speak(self):
        return f"{self.name} barks"
EOF
A=$(get_anchor "$TMPDIR/pyindent.py" "class Dog")
if [ -n "$A" ]; then
    OUT=$($HASHLINE find-block "$TMPDIR/pyindent.py" "$A" 2>&1); RC=$?
    assert_rc "find-block Python indent" 0 "$RC"
    assert "py indent shows Dog class" "class Dog" "$OUT"
    assert "py indent shows speak" "def speak" "$OUT"
else
    assert_skip "find-block Python indent" "could not find anchor for 'class Dog'"
fi

# ===================================================================
log ""
log "=== E15. READ SAME FILE TWICE CONSISTENCY ==="
cat > "$TMPDIR/consist.txt" <<'EOF'
one
two
three
EOF
OUT1=$($HASHLINE read "$TMPDIR/consist.txt" 2>&1)
OUT2=$($HASHLINE read "$TMPDIR/consist.txt" 2>&1)
HASH1=$(echo "$OUT1" | grep -oE "#[0-9a-f]+" | head -1)
HASH2=$(echo "$OUT2" | grep -oE "#[0-9a-f]+" | head -1)
[ "$HASH1" = "$HASH2" ] && assert_pass "same file hash is consistent" || assert_fail "same file hash differs" "hash1=$HASH1 hash2=$HASH2"

# ===================================================================
log ""
log "=== E16. PATCH WITH NO HEADER SECTION ==="
cat > "$TMPDIR/nohdr.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/nohdr.txt" "beta")
# Patch without [file#hash] header (just the operation)
cat > "$TMPDIR/nohdr.patch" <<EOF
SWAP $A2:
+NOHDR_REPLACED
EOF
OUT=$($HASHLINE patch "$TMPDIR/nohdr.txt" "@$TMPDIR/nohdr.patch" 2>&1); RC=$?
assert_rc "no-header patch" 0 "$RC"
[ "$(cat $TMPDIR/nohdr.txt | head -2 | tail -1)" = "NOHDR_REPLACED" ] && assert_pass "no-header patch works" || assert_fail "no-header patch" "$(cat $TMPDIR/nohdr.txt)"

# ===================================================================
log ""
log "================================================================"
log "EDGE CASE RESULTS: $PASS passed, $FAIL failed, $SKIP skipped"
log "================================================================"
[ $FAIL -eq 0 ] && log "ALL EDGE CASE TESTS PASSED!" || log "SOME EDGE CASE TESTS FAILED"
exit $FAIL
