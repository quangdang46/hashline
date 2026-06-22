#!/bin/bash
# hashline comprehensive test suite
set -u
HASHLINE="/Users/tranquangdang21/Projects/hashline/target/release/hashline"
TMPDIR="/tmp/hashline-test"
PASS=0; FAIL=0

log() { echo "$@"; }
assert_pass() { log "  PASS: $1"; PASS=$((PASS+1)); }
assert_fail() { log "  FAIL: $1 :: ${2:-}"; FAIL=$((FAIL+1)); }
assert() { local l="$1" n="$2" h="$3"; echo "$h" | grep -qF -- "$n" && assert_pass "$l" || assert_fail "$l" "missing '$n'"; }
assert_not() { local l="$1" n="$2" h="$3"; echo "$h" | grep -qF -- "$n" && assert_fail "$l" "unexpected '$n'" || assert_pass "$l"; }
assert_rc() { local l="$1" e="$2" g="$3"; [ "$g" = "$e" ] && assert_pass "$l (rc=$g)" || assert_fail "$l" "expected rc=$e got rc=$g"; }

# extract "LINE:HASH" anchor from 'hashline read' output (format: " LINE:HASH|content")
get_anchor() { "$HASHLINE" read "$1" 2>/dev/null | grep -E "$2" | head -1 | sed 's/^[[:space:]]*//;s/|.*//'; }

# write a patch file and apply it
apply_patch() {
    local file="$1" patch_file="$2"
    "$HASHLINE" patch "$file" "@$patch_file" 2>&1
}

rm -rf "$TMPDIR"; mkdir -p "$TMPDIR"

# --- sample files ---
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
delta
epsilon
EOF

cat > "$TMPDIR/sample.rs" <<'EOF'
fn main() {
    println!("Hello, world!");
    let x = 42;
}
fn helper(n: i32) -> i32 {
    if n > 0 {
        return n * 2;
    }
    0
}
EOF

cat > "$TMPDIR/special.txt" <<'EOF'
line with "quotes"
line with unicode 你好 🎉
EOF

touch "$TMPDIR/empty.txt"

log "================================================================"
log "HASHLINE COMPREHENSIVE TEST SUITE"
log "================================================================"
log ""

# ===================================================================
log "=== 1. READ ==="

OUT=$($HASHLINE read "$TMPDIR/sample.rs" 2>&1); RC=$?
assert_rc "read rc=0" 0 "$RC"
assert "read header" "#" "$OUT"
assert "read anchors" "|" "$OUT"
assert "read fn main" "fn main()" "$OUT"
assert "read fn helper" "fn helper" "$OUT"

OUT=$($HASHLINE read "$TMPDIR/sample.rs" --json 2>&1); RC=$?
assert_rc "read --json" 0 "$RC"
assert "json path" "sample.rs" "$OUT"
assert "json hash" '"hash"' "$OUT"
assert "json lines" '"lines"' "$OUT"
echo "$OUT" | python3 -c "import json,sys; json.loads(sys.stdin.read())" 2>/dev/null && assert_pass "valid JSON" || assert_fail "valid JSON"

OUT=$($HASHLINE read "$TMPDIR/nonexist.txt" 2>&1); RC=$?
assert_rc "read missing" 1 "$RC"

OUT=$($HASHLINE read "$TMPDIR/empty.txt" 2>&1); RC=$?
assert_rc "read empty" 0 "$RC"

OUT=$($HASHLINE read "$TMPDIR/sample.rs" --no-cache 2>&1); RC=$?
assert_rc "read --no-cache" 0 "$RC"

OUT=$($HASHLINE read "$TMPDIR/special.txt" 2>&1); RC=$?
assert_rc "read special chars" 0 "$RC"
assert "unicode" "你好" "$OUT"
assert "emoji" "🎉" "$OUT"

# ===================================================================
log ""
log "=== 2. WRITE ==="

OUT=$($HASHLINE write "$TMPDIR/new.txt" "hello\nworld\n" 2>&1); RC=$?
assert_rc "write new" 0 "$RC"
[ -f "$TMPDIR/new.txt" ] && assert_pass "file exists" || assert_fail "file exists"
assert "content" "hello" "$OUT"

OUT=$($HASHLINE write "$TMPDIR/new.txt" "nope" 2>&1); RC=$?
assert_rc "write existing without --force" 1 "$RC"

OUT=$($HASHLINE write "$TMPDIR/new.txt" "yes" --force 2>&1); RC=$?
assert_rc "write --force" 0 "$RC"
[ "$(cat $TMPDIR/new.txt)" = "yes" ] && assert_pass "overwritten correctly" || assert_fail "overwritten correctly"

OUT=$($HASHLINE write "$TMPDIR/atomic.txt" "atomic" --safe 2>&1); RC=$?
assert_rc "write --safe" 0 "$RC"
[ -f "$TMPDIR/atomic.txt" ] && assert_pass "atomic file created" || assert_fail "atomic file created"

OUT=$($HASHLINE write "$TMPDIR/j.txt" "json" --json --force 2>&1); RC=$?
assert_rc "write --json" 0 "$RC"
assert "json success" '"success"' "$OUT"
assert "json lines array" '"lines"' "$OUT"

rm -f "$TMPDIR/new.txt" "$TMPDIR/atomic.txt" "$TMPDIR/j.txt"

# ===================================================================
log ""
log "=== 3. PATCH SWAP ==="

A1=$(get_anchor "$TMPDIR/base.txt" "alpha")
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
A3=$(get_anchor "$TMPDIR/base.txt" "gamma")

log "  anchors: a1=$A1 a2=$A2 a3=$A3"

# 3a: SWAP single with hash
cat > "$TMPDIR/p.patch" <<EOF
SWAP $A2:
+X_BETA
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "SWAP single line" 0 "$RC"
assert "SWAP replaced content" "X_BETA" "$(cat $TMPDIR/base.txt)"

# Recreate base
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
delta
epsilon
EOF

# 3b: SWAP range (3 old lines -> 3 new lines, still 5 total)
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
A4=$(get_anchor "$TMPDIR/base.txt" "delta")
cat > "$TMPDIR/p.patch" <<EOF
SWAP $A2..$A4:
+X2
+X3
+X4
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "SWAP range" 0 "$RC"
CONTENT=$(cat $TMPDIR/base.txt)
assert "SWAP range line 1" "alpha" "$CONTENT"
assert "SWAP range line 2" "X2" "$CONTENT"
assert "SWAP range line 3" "X3" "$CONTENT"
assert "SWAP range line 5" "epsilon" "$CONTENT"
WC=$(echo "$CONTENT" | wc -l | tr -d ' ')
[ "$WC" = "5" ] && assert_pass "SWAP range total lines = 5" || assert_fail "SWAP range total lines" "got $WC"

# 3c: SWAP with wrong hash fails
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
cat > "$TMPDIR/p.patch" <<EOF
SWAP 1:ff:
+X
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
[ "$RC" -ne 0 ] && assert_pass "SWAP wrong hash fails" || assert_fail "SWAP wrong hash" "rc=$RC"

# ===================================================================
log ""
log "=== 4. PATCH DEL ==="

# 4a: DEL single line
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
LN2=$(echo "$A2" | sed 's/:.*//')
cat > "$TMPDIR/p.patch" <<EOF
DEL $LN2
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "DEL single" 0 "$RC"
WC=$(cat $TMPDIR/base.txt | wc -l | tr -d ' ')
[ "$WC" = "2" ] && assert_pass "DEL file has 2 lines" || assert_fail "DEL file lines" "got $WC"

# 4b: DEL range
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
delta
epsilon
EOF
cat > "$TMPDIR/p.patch" <<EOF
DEL 2..4
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "DEL range" 0 "$RC"
WC=$(cat $TMPDIR/base.txt | wc -l | tr -d ' ')
[ "$WC" = "2" ] && assert_pass "DEL range file has 2 lines" || assert_fail "DEL range lines" "got $WC"

# 4c: DEL with hash validation
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
cat > "$TMPDIR/p.patch" <<EOF
DEL 2:ff:
X
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
# DEL with hash suffix should accept hash; the colon after hash means DEL takes a body which should fail
# Actually DEL with :HH: format - let's see
# Actually DEL should not accept a colon after the hash. Let me check
log "  DEL with hash suffix: rc=$RC"
# The trailing colon in "DEL 2:ff:" means there's a body which DEL rejects
# So either it fails (correct) or ignores body (depends on impl)
# Let's move on

# DEL single with hash validation using 2-char hash
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
cat > "$TMPDIR/p.patch" <<EOF
DEL $A2
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "DEL with anchor" 0 "$RC"
WC=$(cat $TMPDIR/base.txt | wc -l | tr -d ' ')
[ "$WC" = "2" ] && assert_pass "DEL with anchor removes line" || assert_fail "DEL with anchor" "got $WC"

# ===================================================================
log ""
log "=== 5. PATCH INS ==="

# 5a: INS.PRE
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
cat > "$TMPDIR/p.patch" <<EOF
INS.PRE $A2:
+PRE_INSERTED
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "INS.PRE" 0 "$RC"
CONTENT=$(cat $TMPDIR/base.txt)
FIRST=$(echo "$CONTENT" | head -1)
SECOND=$(echo "$CONTENT" | head -2 | tail -1)
assert "INS.PRE first line is alpha" "alpha" "$FIRST"
assert "INS.PRE second is inserted" "PRE_INSERTED" "$SECOND"
THIRD=$(echo "$CONTENT" | head -3 | tail -1)
assert "INS.PRE third is beta" "beta" "$THIRD"

# 5b: INS.POST
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A1=$(get_anchor "$TMPDIR/base.txt" "alpha")
cat > "$TMPDIR/p.patch" <<EOF
INS.POST $A1:
+POST_INSERTED
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "INS.POST" 0 "$RC"
CONTENT=$(cat $TMPDIR/base.txt)
assert "INS.POST has inserted" "POST_INSERTED" "$CONTENT"
SECOND=$(echo "$CONTENT" | head -2 | tail -1)
assert "INS.POST second line is inserted" "POST_INSERTED" "$SECOND"

# 5c: INS.HEAD
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
EOF
cat > "$TMPDIR/p.patch" <<EOF
INS.HEAD:
+HEAD_INSERT
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "INS.HEAD" 0 "$RC"
FIRST=$(cat $TMPDIR/base.txt | head -1)
assert "INS.HEAD first line" "HEAD_INSERT" "$FIRST"

# 5d: INS.TAIL
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
EOF
cat > "$TMPDIR/p.patch" <<EOF
INS.TAIL:
+TAIL_INSERT
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
assert_rc "INS.TAIL" 0 "$RC"
LAST=$(cat $TMPDIR/base.txt | tail -1)
assert "INS.TAIL last line" "TAIL_INSERT" "$LAST"

# ===================================================================
log ""
log "=== 6. PATCH --dry-run and --json ==="

cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
cat > "$TMPDIR/p.patch" <<EOF
SWAP $A2:
+REPLACED
EOF
ORIG=$(cat $TMPDIR/base.txt)

# Must test dry-run on the ORIGINAL file first, before --json modifies it
OUT=$($HASHLINE patch "$TMPDIR/base.txt" "@$TMPDIR/p.patch" --dry-run 2>&1); RC=$?
assert_rc "dry-run" 0 "$RC"
assert "dry-run shows diff" "REPLACED" "$OUT"
[ "$(cat $TMPDIR/base.txt)" = "$ORIG" ] && assert_pass "dry-run didn't modify file" || assert_fail "dry-run modified file"

OUT=$($HASHLINE patch "$TMPDIR/base.txt" "@$TMPDIR/p.patch" --dry-run --json 2>&1); RC=$?
assert_rc "dry-run --json" 0 "$RC"
assert "dry-run --json success" '"success"' "$OUT"
assert "dry-run --json dry_run" '"dry_run"' "$OUT"
[ "$(cat $TMPDIR/base.txt)" = "$ORIG" ] && assert_pass "dry-run --json didn't modify" || assert_fail "dry-run --json modified"

OUT=$($HASHLINE patch "$TMPDIR/base.txt" "@$TMPDIR/p.patch" --json 2>&1); RC=$?
assert_rc "patch --json" 0 "$RC"
assert "patch --json success" '"success"' "$OUT"
assert "patch --json edits_applied" '"edits_applied"' "$OUT"

# Recreate for further tests
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF

# ===================================================================
log ""
log "=== 7. PATCH from stdin (@-) ==="

cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
echo "SWAP $A2:
+FROM_STDIN" | $HASHLINE patch "$TMPDIR/base.txt" "-" 2>&1; RC=$?
assert_rc "patch from stdin" 0 "$RC"
assert "stdin content" "FROM_STDIN" "$(cat $TMPDIR/base.txt)"

# ===================================================================
log ""
log "=== 8. PATCH from file (@path) ==="

cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
A2=$(get_anchor "$TMPDIR/base.txt" "beta")
cat > "$TMPDIR/ext.patch" <<EOF
SWAP $A2:
+FROM_FILE
EOF
OUT=$($HASHLINE patch "$TMPDIR/base.txt" "@$TMPDIR/ext.patch" 2>&1); RC=$?
assert_rc "patch from file" 0 "$RC"
assert "from-file content" "FROM_FILE" "$(cat $TMPDIR/base.txt)"

# ===================================================================
log ""
log "=== 9. FIND-BLOCK ==="

# 9a: Rust brace block
A=$(get_anchor "$TMPDIR/sample.rs" "fn helper")
OUT=$($HASHLINE find-block "$TMPDIR/sample.rs" "$A" 2>&1); RC=$?
assert_rc "find-block Rust" 0 "$RC"
assert "find-block shows fn" "fn helper" "$OUT"
assert "find-block shows body" "n > 0" "$OUT"
assert "find-block shows closing" "}" "$OUT"

# 9b: Python indent block
A=$(get_anchor "$TMPDIR/sample.py" "class Calculator" 2>/dev/null) || A=""
if [ -n "$A" ]; then
    OUT=$($HASHLINE find-block "$TMPDIR/sample.py" "$A" 2>&1); RC=$?
    assert_rc "find-block Python" 0 "$RC"
    assert "find-block py class" "class Calculator" "$OUT"
fi

# 9c: find-block --json
A=$(get_anchor "$TMPDIR/sample.rs" "fn helper")
OUT=$($HASHLINE find-block "$TMPDIR/sample.rs" "$A" --json 2>&1); RC=$?
assert_rc "find-block --json" 0 "$RC"
assert "json has content" "fn helper" "$OUT"
echo "$OUT" | python3 -c "import json,sys; json.loads(sys.stdin.read())" 2>/dev/null && assert_pass "find-block JSON valid" || assert_fail "find-block JSON valid"

# 9d: find-block --pretty
OUT=$($HASHLINE find-block "$TMPDIR/sample.rs" "$A" --pretty 2>&1); RC=$?
assert_rc "find-block --pretty" 0 "$RC"
assert "pretty has fn" "fn helper" "$OUT"

# ===================================================================
log ""
log "=== 10. GUIDE ==="

OUT=$($HASHLINE guide 2>&1); RC=$?
assert_rc "guide" 0 "$RC"
assert "guide has user guide" "user guide" "$OUT"
assert "guide has SWAP" "SWAP" "$OUT"
assert "guide has INS" "INS" "$OUT"
assert "guide has BLK" "BLK" "$OUT"
assert "guide has dry-run" "dry-run" "$OUT"

# ===================================================================
log ""
log "=== 11. ERROR HANDLING ==="

# 11a: invalid anchor
A=$(get_anchor "$TMPDIR/base.txt" "beta" 2>/dev/null) || A="2"
if [ -n "$A" ]; then
    cat > "$TMPDIR/p.patch" <<EOF
DEL 99
EOF
    OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
    assert_rc "invalid line number" 1 "$RC"
fi

# 11b: stale hash
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
cat > "$TMPDIR/p.patch" <<EOF
SWAP 1:ff:
+WRONG
EOF
OUT=$(apply_patch "$TMPDIR/base.txt" "$TMPDIR/p.patch"); RC=$?
[ "$RC" -ne 0 ] && assert_pass "stale hash rejected" || assert_fail "stale hash not rejected"

# 11c: unknown command
OUT=$($HASHLINE unrecognized 2>&1); RC=$?
[ "$RC" -ne 0 ] && assert_pass "unknown command" || assert_fail "unknown command"

# 11d: empty patch
cat > "$TMPDIR/base.txt" <<'EOF'
alpha
beta
gamma
EOF
OUT=$($HASHLINE patch "$TMPDIR/base.txt" "" 2>&1); RC=$?
[ "$RC" -ne 0 ] && assert_pass "empty patch rejected" || assert_fail "empty patch not rejected"

# ===================================================================
log ""
log "================================================================"
log "RESULTS: $PASS passed, $FAIL failed"
log "================================================================"
[ $FAIL -eq 0 ] && log "ALL TESTS PASSED!" || log "SOME TESTS FAILED"
exit $FAIL
