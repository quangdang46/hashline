//! Line-based 3-way text merge for stale-anchor recovery.
//!
//! See [`merge_texts`] for the public entry point.

/// The result of a 3-way merge.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// The merged text output.
    pub result: String,
    /// How many conflict regions the merge produced.
    pub conflict_count: usize,
}

/// Perform a line-based 3-way merge.
///
/// * `base`    — the original text (expected-old)
/// * `target`  — the intended new text (what the user wants)
/// * `current` — the actual text on disk (may have diverged from base)
pub fn merge_texts(base: &str, target: &str, current: &str) -> MergeResult {
    let base_lines: Vec<&str> = base.split('\n').collect();
    let target_lines: Vec<&str> = target.split('\n').collect();
    let current_lines: Vec<&str> = current.split('\n').collect();

    let bt_ops = diff_ops(&base_lines, &target_lines);
    let bc_ops = diff_ops(&base_lines, &current_lines);

    let segs_bt = group_segments(&bt_ops);
    let segs_bc = group_segments(&bc_ops);

    merge_segments(&target_lines, &current_lines, &segs_bt, &segs_bc)
}

// ---------------------------------------------------------------------------
// Diff (LCS-based)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum DiffOp {
    Equal(usize),
    Delete(usize),
    Insert(usize),
}

fn diff_ops(old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut o_end = old.len();
    let mut n_end = new.len();
    while o_end > prefix && n_end > prefix && old[o_end - 1] == new[n_end - 1] {
        o_end -= 1;
        n_end -= 1;
    }
    let mut ops = Vec::new();
    if prefix > 0 {
        ops.push(DiffOp::Equal(prefix));
    }
    let mid_old = &old[prefix..o_end];
    let mid_new = &new[prefix..n_end];
    match (mid_old.len(), mid_new.len()) {
        (0, 0) => {}
        (_, 0) => ops.push(DiffOp::Delete(mid_old.len())),
        (0, _) => ops.push(DiffOp::Insert(mid_new.len())),
        (m, n) if m * n <= 200_000 => lcs_diff(mid_old, mid_new, &mut ops),
        _ => {
            ops.push(DiffOp::Delete(mid_old.len()));
            ops.push(DiffOp::Insert(mid_new.len()));
        }
    }
    let suffix = old.len() - o_end;
    if suffix > 0 {
        ops.push(DiffOp::Equal(suffix));
    }
    merge_adjacent(&mut ops);
    ops
}

fn lcs_diff(old: &[&str], new: &[&str], ops: &mut Vec<DiffOp>) {
    let m = old.len();
    let n = new.len();
    if m == 0 {
        ops.push(DiffOp::Insert(n));
        return;
    }
    if n == 0 {
        ops.push(DiffOp::Delete(m));
        return;
    }
    let mut dp: Vec<Vec<u32>> = vec![vec![0u32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut i = 0;
    let mut j = 0;
    while i < m && j < n {
        if old[i] == new[j] {
            let mut cnt = 1;
            while i + cnt < m && j + cnt < n && old[i + cnt] == new[j + cnt] {
                cnt += 1;
            }
            ops.push(DiffOp::Equal(cnt));
            i += cnt;
            j += cnt;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(DiffOp::Delete(1));
            i += 1;
        } else {
            ops.push(DiffOp::Insert(1));
            j += 1;
        }
    }
    if i < m {
        ops.push(DiffOp::Delete(m - i));
    }
    if j < n {
        ops.push(DiffOp::Insert(n - j));
    }
}

fn merge_adjacent(ops: &mut Vec<DiffOp>) {
    if ops.len() < 2 {
        return;
    }
    let mut j = 0usize;
    for i in 1..ops.len() {
        let m = match (ops[j], ops[i]) {
            (DiffOp::Equal(a), DiffOp::Equal(b)) => Some(DiffOp::Equal(a + b)),
            (DiffOp::Delete(a), DiffOp::Delete(b)) => Some(DiffOp::Delete(a + b)),
            (DiffOp::Insert(a), DiffOp::Insert(b)) => Some(DiffOp::Insert(a + b)),
            _ => None,
        };
        if let Some(op) = m {
            ops[j] = op;
        } else {
            j += 1;
            ops[j] = ops[i];
        }
    }
    ops.truncate(j + 1);
}

// ---------------------------------------------------------------------------
// Segments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum SegKind {
    Keep,
    Drop,
    Replace,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    kind: SegKind,
    /// Number of base lines consumed (0 for standalone Insert).
    base_len: usize,
    /// Number of replacement/inserted lines.
    ins: usize,
}

/// Group DiffOps into Segments. A Delete immediately followed by an Insert
/// becomes a Replace. A standalone Insert becomes Replace(0, ins).
fn group_segments(ops: &[DiffOp]) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        match ops[i] {
            DiffOp::Equal(n) => segs.push(Segment {
                kind: SegKind::Keep,
                base_len: n,
                ins: 0,
            }),
            DiffOp::Delete(n) => {
                if i + 1 < ops.len() {
                    if let DiffOp::Insert(m) = ops[i + 1] {
                        segs.push(Segment {
                            kind: SegKind::Replace,
                            base_len: n,
                            ins: m,
                        });
                        i += 1;
                    } else {
                        segs.push(Segment {
                            kind: SegKind::Drop,
                            base_len: n,
                            ins: 0,
                        });
                    }
                } else {
                    segs.push(Segment {
                        kind: SegKind::Drop,
                        base_len: n,
                        ins: 0,
                    });
                }
            }
            DiffOp::Insert(n) => {
                segs.push(Segment {
                    kind: SegKind::Replace,
                    base_len: 0,
                    ins: n,
                });
            }
        }
        i += 1;
    }
    segs
}

// ---------------------------------------------------------------------------
// Merge engine
// ---------------------------------------------------------------------------

fn merge_segments(
    target: &[&str],
    current: &[&str],
    bt: &[Segment],
    bc: &[Segment],
) -> MergeResult {
    let mut out: Vec<String> = Vec::new();
    let mut conflicts = 0usize;
    let mut tp = 0usize;
    let mut cp = 0usize;
    let mut bi = 0usize;
    let mut ci = 0usize;
    let mut bt_rem: Option<Segment> = None;
    let mut bc_rem: Option<Segment> = None;

    loop {
        // -- refill bt --
        if bt_rem.is_none() && bi < bt.len() {
            let s = bt[bi];
            bi += 1;
            bt_rem = Some(s);
        }
        // -- refill bc --
        if bc_rem.is_none() && ci < bc.len() {
            let s = bc[ci];
            ci += 1;
            bc_rem = Some(s);
        }

        // -- handle zero-base-len inserts between the two --
        if let (Some(bs), Some(cs)) = (&bt_rem, &bc_rem) {
            if bs.base_len == 0 && cs.base_len == 0 {
                // Both are inserts at the same position.
                let t = &target[tp..tp + bs.ins];
                let c = &current[cp..cp + cs.ins];
                if t == c && bs.ins == cs.ins {
                    for &l in t {
                        out.push(l.to_string());
                    }
                } else {
                    conflicts += 1;
                    push_conflict(&mut out, t, c);
                }
                tp += bs.ins;
                cp += cs.ins;
                bt_rem = None;
                bc_rem = None;
                continue;
            }
            if bs.base_len == 0 {
                // bt is an insert, bc consumes base.
                for &l in &target[tp..tp + bs.ins] {
                    out.push(l.to_string());
                }
                tp += bs.ins;
                bt_rem = None;
                continue;
            }
            if cs.base_len == 0 {
                // bc is an insert, bt consumes base.
                for &l in &current[cp..cp + cs.ins] {
                    out.push(l.to_string());
                }
                cp += cs.ins;
                bc_rem = None;
                continue;
            }
        }

        let bt_s = match bt_rem.take() {
            Some(s) => s,
            None => {
                // bt exhausted: emit current's remaining segs
                if let Some(s) = bc_rem.take() {
                    emit_current(&mut out, current, &mut cp, &s);
                }
                while ci < bc.len() {
                    let s = bc[ci];
                    ci += 1;
                    if s.base_len == 0 && s.ins > 0 {
                        for &l in &current[cp..cp + s.ins] {
                            out.push(l.to_string());
                        }
                        cp += s.ins;
                    } else {
                        emit_current(&mut out, current, &mut cp, &s);
                    }
                }
                break;
            }
        };

        let bc_s = match bc_rem.take() {
            Some(s) => s,
            None => {
                // bc exhausted: emit target's remaining segs
                emit_target(&mut out, target, &mut tp, &bt_s);
                while bi < bt.len() {
                    let s = bt[bi];
                    bi += 1;
                    if s.base_len == 0 && s.ins > 0 {
                        for &l in &target[tp..tp + s.ins] {
                            out.push(l.to_string());
                        }
                        tp += s.ins;
                    } else {
                        emit_target(&mut out, target, &mut tp, &s);
                    }
                }
                break;
            }
        };

        // -- both present, compare --
        match bt_s.base_len.cmp(&bc_s.base_len) {
            std::cmp::Ordering::Equal => {
                merge_one(
                    &mut out,
                    &mut conflicts,
                    target,
                    current,
                    &mut tp,
                    &mut cp,
                    &bt_s,
                    &bc_s,
                );
            }
            std::cmp::Ordering::Less => {
                bc_rem = Some(Segment {
                    kind: bc_s.kind,
                    base_len: bc_s.base_len - bt_s.base_len,
                    ins: bc_s.ins,
                });
                let bc_front = Segment {
                    kind: bc_s.kind,
                    base_len: bt_s.base_len,
                    ins: 0,
                };
                merge_one(
                    &mut out,
                    &mut conflicts,
                    target,
                    current,
                    &mut tp,
                    &mut cp,
                    &bt_s,
                    &bc_front,
                );
            }
            std::cmp::Ordering::Greater => {
                bt_rem = Some(Segment {
                    kind: bt_s.kind,
                    base_len: bt_s.base_len - bc_s.base_len,
                    ins: bt_s.ins,
                });
                let bt_front = Segment {
                    kind: bt_s.kind,
                    base_len: bc_s.base_len,
                    ins: 0,
                };
                merge_one(
                    &mut out,
                    &mut conflicts,
                    target,
                    current,
                    &mut tp,
                    &mut cp,
                    &bt_front,
                    &bc_s,
                );
            }
        }
    }

    while tp < target.len() {
        out.push(target[tp].to_string());
        tp += 1;
    }
    while cp < current.len() {
        out.push(current[cp].to_string());
        cp += 1;
    }

    MergeResult {
        result: out.join("\n"),
        conflict_count: conflicts,
    }
}

fn emit_target(out: &mut Vec<String>, target: &[&str], tp: &mut usize, seg: &Segment) {
    match seg.kind {
        SegKind::Keep => {
            for &l in &target[*tp..*tp + seg.base_len] {
                out.push(l.to_string());
            }
            *tp += seg.base_len;
        }
        SegKind::Drop => {}
        SegKind::Replace => {
            for &l in &target[*tp..*tp + seg.ins] {
                out.push(l.to_string());
            }
            *tp += seg.ins;
        }
    }
}

fn emit_current(out: &mut Vec<String>, current: &[&str], cp: &mut usize, seg: &Segment) {
    match seg.kind {
        SegKind::Keep => {
            for &l in &current[*cp..*cp + seg.base_len] {
                out.push(l.to_string());
            }
            *cp += seg.base_len;
        }
        SegKind::Drop => {}
        SegKind::Replace => {
            for &l in &current[*cp..*cp + seg.ins] {
                out.push(l.to_string());
            }
            *cp += seg.ins;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_one(
    out: &mut Vec<String>,
    conflicts: &mut usize,
    target: &[&str],
    current: &[&str],
    tp: &mut usize,
    cp: &mut usize,
    bt: &Segment,
    bc: &Segment,
) {
    let blen = bt.base_len;
    match (bt.kind, bc.kind) {
        (SegKind::Keep, SegKind::Keep) => {
            for &l in &target[*tp..*tp + blen] {
                out.push(l.to_string());
            }
            *tp += blen;
            *cp += blen;
        }
        (SegKind::Keep, SegKind::Drop) => {
            *tp += blen;
        }
        (SegKind::Drop, SegKind::Keep) => {
            *cp += blen;
        }
        (SegKind::Drop, SegKind::Drop) => {}
        (SegKind::Keep, SegKind::Replace) => {
            for &l in &current[*cp..*cp + bc.ins] {
                out.push(l.to_string());
            }
            *tp += blen;
            *cp += bc.ins;
        }
        (SegKind::Replace, SegKind::Keep) => {
            for &l in &target[*tp..*tp + bt.ins] {
                out.push(l.to_string());
            }
            *tp += bt.ins;
            *cp += blen;
        }
        (SegKind::Drop, SegKind::Replace) => {
            *conflicts += 1;
            push_conflict(out, &[], &current[*cp..*cp + bc.ins]);
            *cp += bc.ins;
        }
        (SegKind::Replace, SegKind::Drop) => {
            *conflicts += 1;
            push_conflict(out, &target[*tp..*tp + bt.ins], &[]);
            *tp += bt.ins;
        }
        (SegKind::Replace, SegKind::Replace) => {
            let t = &target[*tp..*tp + bt.ins];
            let c = &current[*cp..*cp + bc.ins];
            if t == c && bt.ins == bc.ins && blen == bc.base_len {
                for &l in t {
                    out.push(l.to_string());
                }
            } else {
                *conflicts += 1;
                push_conflict(out, t, c);
            }
            *tp += bt.ins;
            *cp += bc.ins;
        }
    }
}

fn push_conflict(out: &mut Vec<String>, ours: &[&str], theirs: &[&str]) {
    out.push("<<<<<<< target".to_string());
    for &l in ours {
        out.push(l.to_string());
    }
    out.push("=======".to_string());
    for &l in theirs {
        out.push(l.to_string());
    }
    out.push(">>>>>>> current".to_string());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_changes() {
        let r = merge_texts("a\nb\nc", "a\nb\nc", "a\nb\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nb\nc");
    }

    #[test]
    fn test_target_change_only() {
        let r = merge_texts("a\nb\nc", "a\nX\nc", "a\nb\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nX\nc");
    }

    #[test]
    fn test_current_change_only() {
        let r = merge_texts("a\nb\nc", "a\nb\nc", "a\nY\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nY\nc");
    }

    #[test]
    fn test_conflict_same_line() {
        let r = merge_texts("a\nb\nc", "a\nX\nc", "a\nY\nc");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<< target"), "result: {r:?}");
        assert!(r.result.contains(">>>>>>> current"), "result: {r:?}");
    }

    #[test]
    fn test_same_change_no_conflict() {
        let r = merge_texts("a\nb\nc", "a\nX\nc", "a\nX\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nX\nc");
    }

    #[test]
    fn test_independent_changes() {
        let r = merge_texts("a\nb\nc\nd", "a\nX\nc\nd", "a\nb\nY\nd");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nX\nY\nd");
    }

    #[test]
    fn test_conflicting_insertions() {
        let r = merge_texts("a\nc", "a\nX\nc", "a\nY\nc");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<<"));
    }

    #[test]
    fn test_same_insertion() {
        let r = merge_texts("a\nc", "a\nX\nc", "a\nX\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nX\nc");
    }

    #[test]
    fn test_target_delete_only() {
        let r = merge_texts("a\nb\nc", "a\nc", "a\nb\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nc");
    }

    #[test]
    fn test_current_delete_only() {
        let r = merge_texts("a\nb\nc", "a\nb\nc", "a\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nc");
    }

    #[test]
    fn test_both_delete_same() {
        let r = merge_texts("a\nb\nc", "a\nc", "a\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nc");
    }

    #[test]
    fn test_target_multiline_replace() {
        let r = merge_texts("a\nb\nc\nd", "a\nX\nY\nd", "a\nb\nc\nd");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nX\nY\nd");
    }

    #[test]
    fn test_conflict_same_range() {
        let r = merge_texts("a\nb\nc\nd", "a\nX\nY\nd", "a\nP\nQ\nd");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<<"));
    }

    #[test]
    fn test_empty_base() {
        let r = merge_texts("", "a\nb\nc", "");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nb\nc");
    }

    #[test]
    fn test_empty_target() {
        let r = merge_texts("a\nb\nc", "", "");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "");
    }

    #[test]
    fn test_single_line() {
        let r = merge_texts("hello", "world", "hello");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "world");
    }

    #[test]
    fn test_single_line_conflict() {
        let r = merge_texts("hello", "world", "there");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("world"));
        assert!(r.result.contains("there"));
    }

    #[test]
    fn test_target_insert_current_delete() {
        let r = merge_texts("a\nb\nc", "a\nX\nb\nc", "a\nc");
        assert_eq!(r.result, "a\nX\nc");
    }

    #[test]
    fn test_both_append_same() {
        let r = merge_texts("a\nb", "a\nb\nc", "a\nb\nc");
        assert_eq!(r.conflict_count, 0);
        assert_eq!(r.result, "a\nb\nc");
    }

    #[test]
    fn test_both_append_different() {
        let r = merge_texts("a\nb", "a\nb\nc", "a\nb\nd");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<<"));
    }

    #[test]
    fn test_target_drop_current_replace() {
        let r = merge_texts("a\nb\nc", "a\nc", "a\nX\nc");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<<"));
    }

    #[test]
    fn test_target_replace_current_drop() {
        let r = merge_texts("a\nb\nc", "a\nX\nc", "a\nc");
        assert_eq!(r.conflict_count, 1);
        assert!(r.result.contains("<<<<<<<"));
    }
}
