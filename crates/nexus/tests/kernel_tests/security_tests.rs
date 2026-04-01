//! Security & reliability tests.
//!
//! These reproduce vulnerabilities found during the mission-critical audit.
//! Each test documents a specific threat and verifies the kernel handles it safely.
//! If any of these tests are removed or weakened, people may die.

use nexus::*;
use std::fmt;

// --- Minimal test domain ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SId(u64);
impl fmt::Display for SId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for SId {}

#[derive(Debug, Clone)]
enum SEvent {
    Tick,
}
impl Message for SEvent {}
impl DomainEvent for SEvent {
    fn name(&self) -> &'static str {
        "Tick"
    }
}

#[derive(Default, Debug)]
struct SState {
    count: u64,
}
impl AggregateState for SState {
    type Event = SEvent;
    fn apply(&mut self, _: &SEvent) {
        self.count = self.count.wrapping_add(1);
    }
    fn name(&self) -> &'static str {
        "S"
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
#[should_panic(expected = "overflow")]
fn c1_version_next_at_max_must_not_wrap() {
    let v = Version::from_persisted(u64::MAX);
    let _ = v.next(); // MUST panic, not silently wrap to 0
}

#[test]
#[should_panic(expected = "overflow")]
fn c1_current_version_overflow_must_not_wrap() {
    // Version at MAX — next() must panic, not wrap
    let v = Version::from_persisted(u64::MAX);
    let _ = v.next(); // MUST panic, same as c1 above but named for current_version context
}

// =============================================================================
// C2: usize as u64 — must not truncate on platforms where usize > 64 bits
// (this test documents the concern; actual truncation only happens on 128-bit)
// =============================================================================

#[test]
fn c2_uncommitted_count_fits_in_u64() {
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
// C3: Unbounded memory growth — uncommitted event limit
// =============================================================================

// Aggregate with tiny limit for testing
#[derive(Debug)]
struct LimitedAgg;
impl Aggregate for LimitedAgg {
    type State = SState;
    type Error = SError;
    type Id = SId;
    const MAX_UNCOMMITTED: usize = 3;
}

#[test]
#[should_panic(expected = "Uncommitted event limit reached")]
fn c3_apply_event_panics_at_limit() {
    let mut agg = AggregateRoot::<LimitedAgg>::new(SId(1));
    agg.apply_event(SEvent::Tick); // 1
    agg.apply_event(SEvent::Tick); // 2
    agg.apply_event(SEvent::Tick); // 3 — at limit
    agg.apply_event(SEvent::Tick); // 4 — MUST panic
}

#[test]
fn c3_take_resets_count_allowing_more() {
    let mut agg = AggregateRoot::<LimitedAgg>::new(SId(1));
    agg.apply_event(SEvent::Tick);
    agg.apply_event(SEvent::Tick);
    agg.apply_event(SEvent::Tick);

    // At limit, but take resets
    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 3);

    // Can apply again
    agg.apply_event(SEvent::Tick);
    agg.apply_event(SEvent::Tick);
    assert_eq!(agg.current_version(), Version::from_persisted(5));
}

#[test]
fn c3_default_limit_is_1024() {
    assert_eq!(SAgg::MAX_UNCOMMITTED, nexus::DEFAULT_MAX_UNCOMMITTED);
    assert_eq!(nexus::DEFAULT_MAX_UNCOMMITTED, 1024);
}

// =============================================================================
// C4: from_persisted bypass — document that it accepts any value
// =============================================================================

#[test]
fn c4_from_persisted_accepts_zero_version() {
    // from_persisted allows Version(0) — which is INITIAL.
    // This is technically valid for rehydration but could be misused.
    let v = Version::from_persisted(0);
    assert_eq!(v, Version::INITIAL);
}

#[test]
fn c4_from_persisted_versioned_event_no_validation() {
    // from_persisted on VersionedEvent performs no validation.
    // A corrupted store could inject version 0 or backwards versions.
    // load_from_events MUST catch this.
    let bad_events = vec![
        VersionedEvent::from_persisted(Version::from_persisted(5), SEvent::Tick),
        VersionedEvent::from_persisted(Version::from_persisted(3), SEvent::Tick), // backwards!
    ];
    let result = AggregateRoot::<SAgg>::load_from_events(SId(1), bad_events);
    assert!(result.is_err(), "load_from_events must reject non-sequential versions");
}

#[test]
fn c4_from_persisted_duplicate_versions_rejected() {
    let bad_events = vec![
        VersionedEvent::from_persisted(Version::from_persisted(1), SEvent::Tick),
        VersionedEvent::from_persisted(Version::from_persisted(1), SEvent::Tick), // duplicate!
    ];
    let result = AggregateRoot::<SAgg>::load_from_events(SId(1), bad_events);
    assert!(result.is_err(), "load_from_events must reject duplicate versions");
}

// =============================================================================
// H2: Error path heap allocation — KernelError contains String
// =============================================================================

#[test]
fn h2_error_contains_stream_id() {
    let events = vec![
        VersionedEvent::from_persisted(Version::from_persisted(1), SEvent::Tick),
        VersionedEvent::from_persisted(Version::from_persisted(3), SEvent::Tick), // gap
    ];
    let err = AggregateRoot::<SAgg>::load_from_events(SId(42), events).unwrap_err();
    match err {
        KernelError::VersionMismatch { stream_id, .. } => {
            // Verify the ID is captured — NO heap allocation (ErrorId is stack-based)
            assert_eq!(format!("{stream_id}"), "42");
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// H5: apply() panic leaves aggregate inconsistent
// =============================================================================

// Event is recorded BEFORE state mutation.
// If apply() panics, the event survives (recoverable via replay).

#[test]
fn h5_event_recorded_before_state_mutation() {
    let mut agg = AggregateRoot::<SAgg>::new(SId(1));
    agg.apply_event(SEvent::Tick);

    // Both happened in the non-panic path
    assert_eq!(agg.state().count, 1);
    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 1);
}

#[test]
fn h5_event_survives_panic_in_apply() {
    use std::panic;

    // Aggregate with a panicking apply
    #[derive(Debug, Clone)]
    enum BombEvent { Safe, Explode }
    impl Message for BombEvent {}
    impl DomainEvent for BombEvent {
        fn name(&self) -> &'static str {
            match self { Self::Safe => "Safe", Self::Explode => "Explode" }
        }
    }

    #[derive(Default, Debug)]
    struct BombState { count: u64 }
    impl AggregateState for BombState {
        type Event = BombEvent;
        fn apply(&mut self, event: &BombEvent) {
            match event {
                BombEvent::Safe => self.count += 1,
                BombEvent::Explode => panic!("state apply panicked"),
            }
        }
        fn name(&self) -> &'static str { "Bomb" }
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

    let mut agg = AggregateRoot::<BombAgg>::new(SId(1));
    agg.apply_event(BombEvent::Safe); // works fine

    // Panicking apply — event should still be in uncommitted
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        agg.apply_event(BombEvent::Explode);
    }));
    assert!(result.is_err(), "apply should have panicked");

    // The event that caused the panic IS recorded (push happened first)
    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 2, "both events should be recorded, including the one that caused panic");
    assert_eq!(events[0].event().name(), "Safe");
    assert_eq!(events[1].event().name(), "Explode");

    // State only reflects the first event (second apply panicked before completing)
    assert_eq!(agg.state().count, 1);
}

// =============================================================================
// H1: Unbounded rehydration — large event count
// =============================================================================

// Aggregate with tiny rehydration limit for testing
#[derive(Debug)]
struct TinyRehydrationAgg;
impl Aggregate for TinyRehydrationAgg {
    type State = SState;
    type Error = SError;
    type Id = SId;
    const MAX_REHYDRATION_EVENTS: usize = 5;
}

#[test]
fn h1_load_from_events_enforces_rehydration_limit() {
    let events: Vec<_> = (1..=6)
        .map(|i| VersionedEvent::from_persisted(Version::from_persisted(i), SEvent::Tick))
        .collect();

    let result = AggregateRoot::<TinyRehydrationAgg>::load_from_events(SId(1), events);
    assert!(result.is_err());
    match result.unwrap_err() {
        KernelError::RehydrationLimitExceeded { max, .. } => {
            assert_eq!(max, 5);
        }
        other => panic!("expected RehydrationLimitExceeded, got {other:?}"),
    }
}

#[test]
fn h1_load_within_limit_succeeds() {
    let events: Vec<_> = (1..=5)
        .map(|i| VersionedEvent::from_persisted(Version::from_persisted(i), SEvent::Tick))
        .collect();

    let agg = AggregateRoot::<TinyRehydrationAgg>::load_from_events(SId(1), events).unwrap();
    assert_eq!(agg.version(), Version::from_persisted(5));
}

#[test]
fn h1_default_rehydration_limit_is_one_million() {
    assert_eq!(SAgg::MAX_REHYDRATION_EVENTS, nexus::DEFAULT_MAX_REHYDRATION_EVENTS);
    assert_eq!(nexus::DEFAULT_MAX_REHYDRATION_EVENTS, 1_000_000);
}

// =============================================================================
// L2: KernelError exhaustive matching — adding variants breaks downstream
// =============================================================================

#[test]
fn l2_kernel_error_variants_are_known() {
    // If a new variant is added to KernelError, this match must be updated.
    // This forces us to consider the impact on downstream code.
    let err = KernelError::VersionMismatch {
        stream_id: nexus::ErrorId::from_display(&"test"),
        expected: Version::INITIAL,
        actual: Version::from_persisted(1),
    };
    match err {
        KernelError::VersionMismatch { .. } => {}
        KernelError::RehydrationLimitExceeded { .. } => {}
        // If you add a variant, add it here and consider #[non_exhaustive]
    }
}

// =============================================================================
// M5: Malicious size_hint on apply_events
// =============================================================================

#[test]
fn m5_apply_events_with_lying_size_hint() {
    // An iterator that claims to have 0 elements but actually has 3.
    // The kernel should still work correctly (just without pre-allocation).
    struct LyingIterator {
        events: Vec<SEvent>,
        pos: usize,
    }
    impl Iterator for LyingIterator {
        type Item = SEvent;
        fn next(&mut self) -> Option<Self::Item> {
            if self.pos < self.events.len() {
                self.pos += 1;
                Some(self.events[self.pos - 1].clone())
            } else {
                None
            }
        }
        fn size_hint(&self) -> (usize, Option<usize>) {
            (0, None) // lies — claims 0 elements
        }
    }

    let mut agg = AggregateRoot::<SAgg>::new(SId(1));
    let iter = LyingIterator {
        events: vec![SEvent::Tick, SEvent::Tick, SEvent::Tick],
        pos: 0,
    };
    agg.apply_events(iter);
    assert_eq!(agg.current_version(), Version::from_persisted(3));
}
