//! `GlobalSeq` — fjall's [`AllPosition`](nexus_store::AllPosition).
//!
//! A store-local global sequence number: every event a producer appends —
//! across *all* of its streams — receives the next `GlobalSeq` at append time,
//! stamped inside fjall's single serialized write-tx and stored as the
//! `events_global` index key. It is the position an all-streams subscription
//! resumes from, the analogue of [`Version`](nexus::Version) within one stream.
//!
//! It lives in `nexus-fjall` (not `nexus-store`) because the `$all` resume
//! position is **adapter-defined** (#266): an embedded store uses a monotonic
//! scalar; a concurrent SQL store needs a commit-ordered composite. `nexus-store`
//! owns only the [`AllPosition`](nexus_store::AllPosition) trait.
//!
//! The sequence is **monotonic but not gapless**: an aborted append may burn
//! values, so consumers must tolerate gaps and never assume `next == prev + 1`.

use core::fmt;
use std::num::NonZeroU64;

/// A store-local global sequence number (always >= 1). See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalSeq(NonZeroU64);

impl GlobalSeq {
    /// The first sequence number (1).
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    /// The next sequence number, or `None` on overflow at `u64::MAX`.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// The underlying integer value. Always >= 1.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// Construct from a `u64`; `None` if `v` is 0. Mirrors [`NonZeroU64::new`].
    #[must_use]
    pub const fn new(v: u64) -> Option<Self> {
        match NonZeroU64::new(v) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }
}

impl fmt::Display for GlobalSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// fjall's `$all` resume position is its `GlobalSeq` scalar.
impl nexus_store::AllPosition for GlobalSeq {}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// `u64` strategy anchoring the project-mandated boundaries (0, 1, MAX-1,
    /// MAX) alongside a uniform interior. The `- 1` are const-folded, so they
    /// do not trip `arithmetic_side_effects`.
    fn u64_with_boundaries() -> impl Strategy<Value = u64> {
        prop_oneof![
            1 => Just(0u64),
            1 => Just(1u64),
            1 => Just(u64::MAX - 1),
            1 => Just(u64::MAX),
            10 => any::<u64>(),
        ]
    }

    #[test]
    fn new_rejects_zero() {
        // 0 is unrepresentable: a GlobalSeq is always >= 1 (NonZeroU64).
        assert_eq!(GlobalSeq::new(0), None);
    }

    #[test]
    fn initial_is_one() {
        assert_eq!(GlobalSeq::INITIAL.as_u64(), 1);
        assert_eq!(GlobalSeq::new(1), Some(GlobalSeq::INITIAL));
    }

    #[test]
    fn next_overflows_at_max() {
        // No successor at the ceiling — the exclusive `$all` resume relies on
        // this to open an empty scan rather than re-read the ceiling event.
        let max = GlobalSeq::new(u64::MAX).unwrap();
        assert_eq!(max.next(), None);
    }

    #[test]
    fn next_one_below_max_reaches_max() {
        let near = GlobalSeq::new(u64::MAX - 1).unwrap();
        assert_eq!(near.next(), GlobalSeq::new(u64::MAX));
    }

    proptest! {
        /// `new` accepts exactly the nonzero values and round-trips through
        /// `as_u64`; only 0 is rejected.
        #[test]
        fn new_round_trips_nonzero(v in u64_with_boundaries()) {
            if let Some(g) = GlobalSeq::new(v) {
                prop_assert_eq!(g.as_u64(), v);
            } else {
                prop_assert_eq!(v, 0);
            }
        }

        /// `Ord` on `GlobalSeq` is exactly `u64` order on the underlying value
        /// — the total order the exclusive resume depends on.
        #[test]
        fn ord_matches_underlying_u64(a in u64_with_boundaries(), b in u64_with_boundaries()) {
            prop_assume!(a != 0 && b != 0);
            let ga = GlobalSeq::new(a).unwrap();
            let gb = GlobalSeq::new(b).unwrap();
            prop_assert_eq!(ga.cmp(&gb), a.cmp(&b));
        }

        /// `next` is the `+1` successor for every value below `u64::MAX`, and
        /// `None` exactly at the ceiling.
        #[test]
        fn next_is_successor_or_none_at_ceiling(v in u64_with_boundaries()) {
            prop_assume!(v != 0);
            let g = GlobalSeq::new(v).unwrap();
            if let Some(n) = g.next() {
                prop_assert_eq!(n.as_u64(), v.checked_add(1).unwrap());
            } else {
                prop_assert_eq!(v, u64::MAX);
            }
        }
    }
}
