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
            // Verify the ID is captured — this allocates a String on the error path
            assert_eq!(stream_id, "42");
        }
    }
}

// =============================================================================
// H5: apply() panic leaves aggregate inconsistent
// =============================================================================

// We can't easily test a panicking apply() without catch_unwind,
// so we document the concern and test the ordering guarantee.

#[test]
fn h5_event_pushed_after_state_apply() {
    // Currently, state.apply() is called BEFORE push to uncommitted_events.
    // If apply() panics, the event is lost but state may be partially mutated.
    // This test verifies the current ordering so any change is intentional.
    let mut agg = AggregateRoot::<SAgg>::new(SId(1));
    agg.apply_event(SEvent::Tick);

    // State was mutated
    assert_eq!(agg.state().count, 1);
    // Event was tracked
    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 1);
    // Both happened — no inconsistency in the non-panic path
}

// =============================================================================
// H1: Unbounded rehydration — large event count
// =============================================================================

#[test]
fn h1_load_from_large_event_count() {
    // Verify load_from_events works with a large but reasonable count.
    // On a constrained device, this should be bounded.
    let n = 10_000;
    let events: Vec<_> = (1..=n)
        .map(|i| VersionedEvent::from_persisted(Version::from_persisted(i), SEvent::Tick))
        .collect();

    let agg = AggregateRoot::<SAgg>::load_from_events(SId(1), events).unwrap();
    assert_eq!(agg.version(), Version::from_persisted(n));
    assert_eq!(agg.state().count, n);
}

// =============================================================================
// L2: KernelError exhaustive matching — adding variants breaks downstream
// =============================================================================

#[test]
fn l2_kernel_error_variants_are_known() {
    // If a new variant is added to KernelError, this match must be updated.
    // This forces us to consider the impact on downstream code.
    let err = KernelError::VersionMismatch {
        stream_id: "test".into(),
        expected: Version::INITIAL,
        actual: Version::from_persisted(1),
    };
    match err {
        KernelError::VersionMismatch { .. } => {}
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
