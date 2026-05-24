// Progress + Step — accumulator types and per-step outcome
// for a checkpointed scan over an event stream.

// ═══════════════════════════════════════════════════════════════════════════
// Progress — accumulator for a checkpointed scan over an event stream
// ═══════════════════════════════════════════════════════════════════════════

/// Accumulator for a checkpointed scan over an event stream.
///
/// Carries the in-memory state plus two cursors: the last position
/// durably saved (`saved`), and the last position seen by the scan
/// (`seen`). The relationship `seen > saved` means the scan has
/// observed events that have not yet been persisted — work the
/// caller may want to flush before yielding the accumulator.
///
/// Generic over both the state type `S` and the position type `P`,
/// so the same shape works for per-stream scans (`P = Version`) and
/// multi-stream subscriptions (`P = GlobalSeq`).
///
/// # Invariants
///
/// - After [`fresh`](Progress::fresh): `saved == seen == None`.
/// - After [`resume`](Progress::resume): `saved == seen == Some(_)`.
/// - The scan body is responsible for advancing `seen` monotonically
///   as it observes events; this type does not enforce that beyond
///   the [`dirty`](Progress::dirty) / [`unsaved`](Progress::unsaved)
///   predicates relying on `Option<P>`'s derived ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress<S, P> {
    /// In-memory accumulator state.
    pub state: S,
    /// Position last durably saved.
    pub saved: Option<P>,
    /// Position last observed by the scan.
    pub seen: Option<P>,
}

impl<S, P> Progress<S, P> {
    /// Cold start — no prior cursor, no state has been observed or saved.
    ///
    /// Both `saved` and `seen` are `None`.
    pub const fn fresh(state: S) -> Self {
        Self {
            state,
            saved: None,
            seen: None,
        }
    }
}

impl<S, P: Copy> Progress<S, P> {
    /// Resume from a previously saved cursor and state.
    ///
    /// Both `saved` and `seen` are set to `saved_at` so the resumed
    /// scan starts in a non-dirty state.
    pub const fn resume(saved_at: P, state: S) -> Self {
        Self {
            state,
            saved: Some(saved_at),
            seen: Some(saved_at),
        }
    }
}

impl<S, P: Copy + Ord> Progress<S, P> {
    /// `true` when the scan has observed positions past the last save.
    ///
    /// Relies on the derived `PartialOrd` for `Option<P>`, where
    /// `None < Some(_)`. So `Some(seen) > None` (saved nothing yet,
    /// observed something) and `Some(seen) > Some(saved)` (observed
    /// beyond the saved point) both return `true`.
    pub fn dirty(&self) -> bool {
        self.seen > self.saved
    }

    /// The position to save at when [`dirty`](Self::dirty), else `None`.
    ///
    /// Returns the value the caller needs to persist alongside `state`
    /// — no separate `unwrap` of `seen` required at the call site.
    pub fn unsaved(&self) -> Option<P> {
        self.dirty().then_some(self.seen).flatten()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Step — per-iteration outcome of a checkpointed scan
// ═══════════════════════════════════════════════════════════════════════════

/// Per-iteration outcome of a checkpointed scan.
///
/// Returned by the scan body to tell the caller what to do next:
/// either persist the carried [`Progress`] before continuing, or
/// carry it forward without IO. The enum (rather than a `bool`)
/// makes the contract a `match` the compiler enforces — leave a
/// variant unhandled and the build fails.
pub enum Step<S, P> {
    /// The scan body's policy fired — caller should durably save the
    /// carried `Progress` (typically at `progress.seen`) before the
    /// next iteration.
    Save(Progress<S, P>),
    /// The scan body's policy did not fire — caller carries the
    /// `Progress` forward without performing any IO.
    Skip(Progress<S, P>),
}
// ═══════════════════════════════════════════════════════════════════════════
// Tests — pure sync coverage for Progress constructors and predicates
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
#[allow(
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test code — relaxed lints"
)]
mod tests {
    use nexus::Version;

    use super::Progress;

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    // ── Progress::fresh — cold start invariants ───────────────────────

    #[test]
    fn fresh_has_no_saved_or_seen() {
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert_eq!(p.state, 0);
        assert!(p.saved.is_none());
        assert!(p.seen.is_none());
    }

    // ── Progress::resume — warm start invariants ──────────────────────

    #[test]
    fn resume_aligns_saved_and_seen() {
        let p: Progress<u64, Version> = Progress::resume(v(7), 42);
        assert_eq!(p.state, 42);
        assert_eq!(p.saved, Some(v(7)));
        assert_eq!(p.seen, Some(v(7)));
    }

    // ── Progress::dirty — three-case truth table ──────────────────────

    #[test]
    fn dirty_is_false_when_seen_is_none() {
        // Nothing observed yet → nothing to flush.
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert!(!p.dirty());
    }

    #[test]
    fn dirty_is_false_when_seen_equals_saved() {
        // Just resumed (or just saved) → aligned, nothing to flush.
        let p: Progress<u64, Version> = Progress::resume(v(3), 0);
        assert!(!p.dirty());
    }

    #[test]
    fn dirty_is_true_when_seen_is_ahead_of_saved() {
        // Saved at None, seen at Some — observed something never saved.
        let mut p: Progress<u64, Version> = Progress::fresh(0);
        p.seen = Some(v(1));
        assert!(p.dirty());

        // Saved at Some(x), seen at Some(y) with y > x — pending events
        // since last save.
        let mut p: Progress<u64, Version> = Progress::resume(v(3), 0);
        p.seen = Some(v(7));
        assert!(p.dirty());
    }

    // ── Progress::unsaved — returns the position to save at ───────────

    #[test]
    fn unsaved_returns_none_when_not_dirty() {
        let p: Progress<u64, Version> = Progress::fresh(0);
        assert!(p.unsaved().is_none());

        let p: Progress<u64, Version> = Progress::resume(v(3), 0);
        assert!(p.unsaved().is_none());
    }

    #[test]
    fn unsaved_returns_seen_when_dirty() {
        let mut p: Progress<u64, Version> = Progress::resume(v(3), 0);
        p.seen = Some(v(7));
        assert_eq!(p.unsaved(), Some(v(7)));
    }
}
