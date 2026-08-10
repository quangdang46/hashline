//! No-op loop guard.
//!
//! Prevents an agent (or its harness) from repeatedly issuing the *same*
//! patch that produces *no net change* — the "apply a no-op forever" loop.
//! After `threshold` consecutive identical no-ops on the same path, the guard
//! returns a hard error so the caller re-reads and re-anchors instead of
//! spinning.
//!
//! A no-op is defined as: same path + same patch fingerprint + net-zero
//! content change. Changing either the path, the patch text, or the resulting
//! content resets the counter.

use std::collections::HashMap;

/// Default number of consecutive identical no-ops before the guard fires
/// (mirrors pi-hashline-edit's `NOOP_HARD_LIMIT = 3`).
pub const DEFAULT_NOOP_LIMIT: usize = 3;

/// Tracks consecutive identical no-op patches per path.
#[derive(Default, Debug)]
pub struct NoopGuard {
    /// path → (patch fingerprint, consecutive identical no-op count)
    state: HashMap<String, (u64, usize)>,
    /// Consecutive identical no-ops before the guard fires.
    limit: usize,
}

impl NoopGuard {
    pub fn new(limit: usize) -> Self {
        Self {
            state: HashMap::new(),
            limit: limit.max(1),
        }
    }

    /// Register an apply result and return the current no-op streak.
    ///
    /// `was_noop` is true when the patch produced no net content change.
    /// `fingerprint` identifies the patch text for the path (see [`fingerprint`]).
    ///
    /// Returns `Ok(streak)` when the guard is satisfied (streak < limit), and
    /// `Err(streak)` when `streak >= limit` — the caller should surface a
    /// hard no-op-loop error.
    pub fn record(
        &mut self,
        path: &str,
        fingerprint: u64,
        was_noop: bool,
    ) -> Result<usize, usize> {
        if !was_noop {
            // A real change (or an empty/no edits path) resets the streak.
            self.state.remove(path);
            return Ok(0);
        }

        let entry = self.state.entry(path.to_owned()).or_insert((fingerprint, 0));
        if entry.0 != fingerprint {
            // Different patch text → new attempt, not a loop.
            *entry = (fingerprint, 1);
        } else {
            entry.1 += 1;
        }

        let streak = entry.1;
        if streak >= self.limit {
            Err(streak)
        } else {
            Ok(streak)
        }
    }

    /// Reset the guard for `path` (e.g. after a successful edit or re-read).
    pub fn reset(&mut self, path: &str) {
        self.state.remove(path);
    }
}

/// A stable fingerprint of a patch for the no-op guard. Two patches that
/// differ in text get different fingerprints (best-effort; collisions just
/// extend the streak, which is safe — the loop guard fails closed).
pub fn fingerprint(patch_str: &str) -> u64 {
    crate::hash::full_hash_bytes64(patch_str.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_loop_fires_after_three_identical() {
        let mut g = NoopGuard::new(3);
        let fp = fingerprint("SWAP 2:\n+same");

        assert_eq!(g.record("a.txt", fp, true), Ok(1));
        assert_eq!(g.record("a.txt", fp, true), Ok(2));
        assert_eq!(g.record("a.txt", fp, true), Err(3));
    }

    #[test]
    fn real_change_resets_streak() {
        let mut g = NoopGuard::new(3);
        let fp = fingerprint("SWAP 2:\n+same");

        assert_eq!(g.record("a.txt", fp, true), Ok(1));
        // A real change (was_noop=false) resets the streak.
        assert_eq!(g.record("a.txt", fp, false), Ok(0));
        assert_eq!(g.record("a.txt", fp, true), Ok(1));
    }

    #[test]
    fn different_patch_resets_streak() {
        let mut g = NoopGuard::new(3);
        assert_eq!(g.record("a.txt", fingerprint("SWAP 2:\n+x"), true), Ok(1));
        assert_eq!(g.record("a.txt", fingerprint("SWAP 3:\n+y"), true), Ok(1));
        assert_eq!(g.record("a.txt", fingerprint("DEL 2"), true), Ok(1));
    }

    #[test]
    fn different_path_independent() {
        let mut g = NoopGuard::new(3);
        let fp = fingerprint("SWAP 2:\n+same");
        assert_eq!(g.record("a.txt", fp, true), Ok(1));
        assert_eq!(g.record("b.txt", fp, true), Ok(1));
        assert_eq!(g.record("b.txt", fp, true), Ok(2));
    }

    #[test]
    fn reset_clears_path() {
        let mut g = NoopGuard::new(3);
        let fp = fingerprint("SWAP 2:\n+same");
        assert_eq!(g.record("a.txt", fp, true), Ok(1));
        g.reset("a.txt");
        assert_eq!(g.record("a.txt", fp, true), Ok(1));
    }
}
