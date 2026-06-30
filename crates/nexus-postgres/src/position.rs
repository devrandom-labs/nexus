//! `PgAllPos` — postgres's [`AllPosition`](nexus_store::AllPosition).
//!
//! The composite `(txid, seq)` commit-ordered position. `txid` is the writing
//! transaction's `xid8` (`pg_current_xact_id()`); `seq` is the row's
//! `IDENTITY` `global_seq`. Lexicographic `Ord` on `(txid, seq)` is the
//! Way-2 order: resume strictly after the last delivered position, and the
//! `pg_snapshot_xmin` watermark guarantees no smaller `(txid, seq)` can become
//! visible after a larger one is consumed.

/// `PostgreSQL` `$all` resume position: `(txid, seq)`, lexicographic order.
///
/// Fields are **private** — the `(u64, u64)` layout is an implementation
/// detail, not the public contract. A consumer checkpoints a `PgAllPos` and
/// hands it back to resume; it reconstructs one from its two persisted scalars
/// via [`new`](Self::new) and reads them via [`txid`](Self::txid) /
/// [`seq`](Self::seq), but it cannot mutate one in place or depend on the field
/// layout. That representation-independence is what lets the position evolve
/// after the 1.0 freeze (the public surface is `new` + two accessors + `Ord`,
/// not "a struct with two pub u64s").
///
/// # Ordering
///
/// `derive(Ord)` uses **declaration order**: `txid` first, then `seq`. This
/// matches `ORDER BY txid, global_seq` in SQL and the row-value comparison
/// `(txid, global_seq) > ($1, $2)`. A sequence/protocol test asserts
/// `PgAllPos::new(1, 9) < PgAllPos::new(2, 0)` (lower `txid` wins regardless
/// of `seq`) to lock the tie-break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PgAllPos {
    /// The writing transaction id (`xid8`, 64-bit, non-wrapping). Declaration
    /// order is the `derive(Ord)` primary key — `txid` sorts before `seq`.
    txid: u64,
    /// The row's `IDENTITY` `global_seq` (always >= 1).
    seq: u64,
}

impl PgAllPos {
    /// Construct from the `(txid, seq)` columns read off a row — or rebuilt
    /// from a persisted consumer checkpoint.
    #[must_use]
    pub const fn new(txid: u64, seq: u64) -> Self {
        Self { txid, seq }
    }

    /// The writing transaction id component (serialize this to checkpoint).
    #[must_use]
    pub const fn txid(self) -> u64 {
        self.txid
    }

    /// The `global_seq` component (serialize this to checkpoint).
    #[must_use]
    pub const fn seq(self) -> u64 {
        self.seq
    }
}

impl nexus_store::AllPosition for PgAllPos {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sequence/protocol: lexicographic `Ord` — `txid` is the primary sort key.
    /// `PgAllPos::new(1, 9)` must be less than `PgAllPos::new(2, 0)` regardless
    /// of `seq`, because this must match `ORDER BY txid, global_seq` in SQL.
    #[test]
    fn ord_is_txid_first_then_seq() {
        // Lower txid wins regardless of seq.
        assert!(PgAllPos::new(1, 9) < PgAllPos::new(2, 0));
        // Equal txid: seq is the tiebreaker.
        assert!(PgAllPos::new(3, 1) < PgAllPos::new(3, 2));
        // Equality.
        assert_eq!(PgAllPos::new(5, 5), PgAllPos::new(5, 5));
    }

    /// `new` / `txid` / `seq` round-trip correctly.
    #[test]
    fn accessors_round_trip() {
        let pos = PgAllPos::new(42, 7);
        assert_eq!(pos.txid(), 42);
        assert_eq!(pos.seq(), 7);
    }
}
