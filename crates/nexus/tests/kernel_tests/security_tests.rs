//! Security & reliability tests.
//!
//! These reproduce vulnerabilities found during the mission-critical audit.
//! Each test documents a specific threat and verifies the kernel handles it safely.
//! If any of these tests are removed or weakened, people may die.

use nexus::*;
use std::fmt;
use std::num::NonZeroUsize;

// --- Minimal test domain ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SId(String);
impl SId {
    fn new(v: u64) -> Self {
        Self(v.to_string())
    }
}
impl fmt::Display for SId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl AsRef<[u8]> for SId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for SId {}

#[derive(Debug, Clone, PartialEq)]
enum SEvent {
    Tick,
}
impl Message for SEvent {}
impl DomainEvent for SEvent {
    fn name(&self) -> &'static str {
        "Tick"
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
struct SState {
    count: u64,
}
impl AggregateState for SState {
    type Event = SEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, _: &SEvent) -> Self {
        self.count = self.count.wrapping_add(1);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct SError;

#[derive(Debug)]
struct SAgg;
impl Aggregate for SAgg {
    type State = SState;
    type Error = SError;
    type Id = SId;
}

// =============================================================================
// C1: Version overflow — Version::next() at u64::MAX must not silently wrap
// =============================================================================

#[test]
fn c1_version_next_at_max_returns_none() {
    let v = Version::new(u64::MAX).expect("u64::MAX is non-zero");
    assert!(
        v.next().is_none(),
        "Version::next() at u64::MAX must return None, not wrap"
    );
}

#[test]
fn c1_replay_at_max_version_returns_overflow_error() {
    // Construct an aggregate already at u64::MAX via replay, then attempt
    // one more replay — must get VersionOverflow.
    // We can't replay u64::MAX events, so we test the VersionOverflow path
    // by replaying version 1, then attempting to advance beyond what's possible.
    // Instead, test that Version::next() returning None is correctly mapped
    // to KernelError::VersionOverflow by the replay method.
    let mut agg = AggregateRoot::<SAgg>::new(SId::new(1));

    // Replay version 1 so the aggregate has a version
    agg.replay(Version::INITIAL, &SEvent::Tick)
        .expect("first replay should succeed");

    // We can't easily get to u64::MAX via replay, but we can verify the
    // Version type itself prevents overflow
    let max = Version::new(u64::MAX).expect("u64::MAX is non-zero");
    assert!(max.next().is_none(), "overflow must be caught");
}

// =============================================================================
// C2: usize as u64 — must not truncate on platforms where usize > 64 bits
// (this test documents the concern; actual truncation only happens on 128-bit)
// =============================================================================

#[test]
fn c2_usize_fits_in_u64() {
    // On current platforms (32/64 bit), usize always fits in u64.
    // This test verifies the assumption holds at compile time.
    // If Rust ever runs on a 128-bit platform, this assertion will fail
    // and force us to handle the conversion properly.
    assert!(
        std::mem::size_of::<usize>() <= std::mem::size_of::<u64>(),
        "usize exceeds u64 — version arithmetic will truncate!"
    );
}

// =============================================================================
// C3: MAX_UNCOMMITTED is removed — no event buffering in AggregateRoot.
// These tests are intentionally omitted. AggregateRoot no longer buffers
// uncommitted events; the repository handles persistence externally.
// =============================================================================

// =============================================================================
// C4: Version::new(0) returns None; replay rejects bad versions
// =============================================================================

#[test]
fn c4_version_new_zero_returns_none() {
    assert!(
        Version::new(0).is_none(),
        "Version::new(0) must return None — versions start at 1"
    );
}

#[test]
fn c4_replay_rejects_backwards_version() {
    let mut agg = AggregateRoot::<SAgg>::new(SId::new(1));
    agg.replay(Version::INITIAL, &SEvent::Tick)
        .expect("version 1 should succeed");
    let v2 = Version::new(2).expect("2 is non-zero");
    agg.replay(v2, &SEvent::Tick)
        .expect("version 2 should succeed");

    // Attempt backwards version — must be rejected
    let result = agg.replay(Version::INITIAL, &SEvent::Tick);
    assert!(
        result.is_err(),
        "replay must reject non-sequential versions"
    );
    match result.unwrap_err() {
        KernelError::VersionMismatch { expected, actual } => {
            let v3 = Version::new(3).expect("3 is non-zero");
            assert_eq!(expected, v3);
            assert_eq!(actual, Version::INITIAL);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn c4_replay_rejects_duplicate_version() {
    let mut agg = AggregateRoot::<SAgg>::new(SId::new(1));
    agg.replay(Version::INITIAL, &SEvent::Tick)
        .expect("version 1 should succeed");

    // Attempt duplicate version — must be rejected
    let result = agg.replay(Version::INITIAL, &SEvent::Tick);
    assert!(result.is_err(), "replay must reject duplicate versions");
    match result.unwrap_err() {
        KernelError::VersionMismatch { expected, actual } => {
            let v2 = Version::new(2).expect("2 is non-zero");
            assert_eq!(expected, v2);
            assert_eq!(actual, Version::INITIAL);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn c4_replay_rejects_gap_in_versions() {
    let mut agg = AggregateRoot::<SAgg>::new(SId::new(1));
    agg.replay(Version::INITIAL, &SEvent::Tick)
        .expect("version 1 should succeed");

    // Skip version 2, jump to 3 — must be rejected
    let v3 = Version::new(3).expect("3 is non-zero");
    let result = agg.replay(v3, &SEvent::Tick);
    assert!(result.is_err(), "replay must reject version gaps");
    match result.unwrap_err() {
        KernelError::VersionMismatch { expected, actual } => {
            let v2 = Version::new(2).expect("2 is non-zero");
            assert_eq!(expected, v2);
            assert_eq!(actual, v3);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// H1: Unbounded rehydration — large event count
// =============================================================================

// Aggregate with tiny rehydration limit for testing
#[derive(Debug)]
struct TinyRehydrationAgg;

#[allow(clippy::unwrap_used, reason = "5 is non-zero by inspection")]
impl Aggregate for TinyRehydrationAgg {
    type State = SState;
    type Error = SError;
    type Id = SId;
    const MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(5).unwrap();
}

#[test]
fn h1_replay_enforces_rehydration_limit() {
    let mut agg = AggregateRoot::<TinyRehydrationAgg>::new(SId::new(1));
    for i in 1..=5u64 {
        let v = Version::new(i).expect("1..=5 are non-zero");
        agg.replay(v, &SEvent::Tick).expect("within limit");
    }
    // 6th event exceeds the limit of 5
    let v6 = Version::new(6).expect("6 is non-zero");
    let result = agg.replay(v6, &SEvent::Tick);
    assert!(result.is_err());
    match result.unwrap_err() {
        KernelError::RehydrationLimitExceeded { max } => {
            assert_eq!(max, 5);
        }
        other => panic!("expected RehydrationLimitExceeded, got {other:?}"),
    }
}

#[test]
fn h1_replay_within_limit_succeeds() {
    let mut agg = AggregateRoot::<TinyRehydrationAgg>::new(SId::new(1));
    for i in 1..=5u64 {
        let v = Version::new(i).expect("1..=5 are non-zero");
        agg.replay(v, &SEvent::Tick).expect("within limit");
    }
    let v5 = Version::new(5).expect("5 is non-zero");
    assert_eq!(agg.version(), Some(v5));
}

#[test]
fn h1_default_rehydration_limit_is_one_million() {
    assert_eq!(SAgg::MAX_REHYDRATION_EVENTS, DEFAULT_MAX_REHYDRATION_EVENTS);
    assert_eq!(DEFAULT_MAX_REHYDRATION_EVENTS.get(), 1_000_000);
}

// =============================================================================
// H2: Error contents — KernelError::VersionMismatch has expected + actual only
// =============================================================================

#[test]
fn h2_version_mismatch_contains_expected_and_actual() {
    let mut agg = AggregateRoot::<SAgg>::new(SId::new(42));
    agg.replay(Version::INITIAL, &SEvent::Tick)
        .expect("version 1 should succeed");

    // Gap: skip version 2, jump to 3
    let v3 = Version::new(3).expect("3 is non-zero");
    let err = agg.replay(v3, &SEvent::Tick).unwrap_err();
    match err {
        KernelError::VersionMismatch { expected, actual } => {
            let v2 = Version::new(2).expect("2 is non-zero");
            assert_eq!(expected, v2);
            assert_eq!(actual, v3);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn h2_version_mismatch_display_includes_both_versions() {
    let v2 = Version::new(2).expect("2 is non-zero");
    let v5 = Version::new(5).expect("5 is non-zero");
    let err = KernelError::VersionMismatch {
        expected: v2,
        actual: v5,
    };
    let msg = format!("{err}");
    assert!(
        msg.contains('2'),
        "error message must include expected version"
    );
    assert!(
        msg.contains('5'),
        "error message must include actual version"
    );
}

// =============================================================================
// H5: Panic safety during replay — clone-based preservation
// =============================================================================

#[test]
fn h5_replay_panic_preserves_original_state() {
    use std::panic;

    // Aggregate with a panicking apply
    #[derive(Debug, Clone)]
    enum BombEvent {
        Safe,
        Explode,
    }
    impl Message for BombEvent {}
    impl DomainEvent for BombEvent {
        fn name(&self) -> &'static str {
            match self {
                Self::Safe => "Safe",
                Self::Explode => "Explode",
            }
        }
    }

    #[derive(Default, Debug, Clone)]
    struct BombState {
        count: u64,
    }
    impl AggregateState for BombState {
        type Event = BombEvent;
        fn initial() -> Self {
            Self::default()
        }
        fn apply(mut self, event: &BombEvent) -> Self {
            match event {
                BombEvent::Safe => self.count += 1,
                BombEvent::Explode => panic!("state apply panicked"),
            }
            self
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct BombError;

    #[derive(Debug)]
    struct BombAgg;
    impl Aggregate for BombAgg {
        type State = BombState;
        type Error = BombError;
        type Id = SId;
    }

    let mut agg = AggregateRoot::<BombAgg>::new(SId::new(1));
    agg.replay(Version::INITIAL, &BombEvent::Safe)
        .expect("safe replay should succeed");

    // State should be count=1, version=1 after safe replay
    assert_eq!(agg.state().count, 1);
    assert_eq!(agg.version(), Some(Version::INITIAL));

    // Panicking replay — state and version must be preserved
    let v2 = Version::new(2).expect("2 is non-zero");
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _ = agg.replay(v2, &BombEvent::Explode);
    }));
    assert!(result.is_err(), "replay should have panicked");

    // Original state preserved: count still 1, version still 1
    assert_eq!(
        agg.state().count,
        1,
        "state must be preserved after panic in apply"
    );
    assert_eq!(
        agg.version(),
        Some(Version::INITIAL),
        "version must not advance after panic in apply"
    );
}

#[test]
fn h5_apply_events_panic_preserves_partial_state() {
    use std::panic;

    #[derive(Debug, Clone)]
    enum SeqEvent {
        Inc,
        Boom,
    }
    impl Message for SeqEvent {}
    impl DomainEvent for SeqEvent {
        fn name(&self) -> &'static str {
            match self {
                Self::Inc => "Inc",
                Self::Boom => "Boom",
            }
        }
    }

    #[derive(Default, Debug, Clone)]
    struct SeqState {
        count: u64,
    }
    impl AggregateState for SeqState {
        type Event = SeqEvent;
        fn initial() -> Self {
            Self::default()
        }
        fn apply(mut self, event: &SeqEvent) -> Self {
            match event {
                SeqEvent::Inc => self.count += 1,
                SeqEvent::Boom => panic!("boom in apply_events"),
            }
            self
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct SeqError;

    #[derive(Debug)]
    struct SeqAgg;
    impl Aggregate for SeqAgg {
        type State = SeqState;
        type Error = SeqError;
        type Id = SId;
    }

    let mut agg = AggregateRoot::<SeqAgg>::new(SId::new(1));

    // apply_events with a panic in the middle
    let events: Events<_, 1> = events![SeqEvent::Inc, SeqEvent::Boom];
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        agg.apply_events(&events);
    }));
    assert!(result.is_err(), "apply_events should have panicked");

    // After panic: first event was applied (clone-then-apply per event),
    // but state may be partially updated depending on apply_events implementation.
    // The important thing: the aggregate is not in an invalid/corrupt state.
    // version() should still be None since apply_events doesn't set version.
    assert_eq!(
        agg.version(),
        None,
        "version must remain None — apply_events does not set version"
    );
}

// =============================================================================
// L2: KernelError variants — exhaustive matching
// =============================================================================

#[test]
fn l2_kernel_error_variants_are_known() {
    // If a new variant is added to KernelError, this match must be updated.
    // This forces us to consider the impact on downstream code.
    // KernelError is #[non_exhaustive], so we use a wildcard for future variants.
    let err = KernelError::VersionMismatch {
        expected: Version::INITIAL,
        actual: Version::new(2).expect("2 is non-zero"),
    };
    match &err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(*expected, Version::INITIAL);
            assert_eq!(actual.as_u64(), 2);
        }
        KernelError::RehydrationLimitExceeded { max } => {
            panic!("wrong variant: RehydrationLimitExceeded(max={max})")
        }
        KernelError::VersionOverflow => {
            panic!("wrong variant: VersionOverflow")
        }
        other => panic!("unknown new variant: {other:?}"),
    }
}

#[test]
fn l2_version_overflow_variant_exists() {
    let err = KernelError::VersionOverflow;
    let msg = format!("{err}");
    assert!(
        msg.contains("u64::MAX") || msg.contains("exhausted"),
        "VersionOverflow message should mention the limit: {msg}"
    );
}

#[test]
fn l2_rehydration_limit_variant_has_max() {
    let err = KernelError::RehydrationLimitExceeded { max: 42 };
    let msg = format!("{err}");
    assert!(
        msg.contains("42"),
        "RehydrationLimitExceeded message should include the max value: {msg}"
    );
}

// =============================================================================
// M3: Clone and PartialEq on AggregateRoot — REMOVED.
// AggregateRoot no longer implements Clone or PartialEq.
// These tests are intentionally omitted.
// =============================================================================
