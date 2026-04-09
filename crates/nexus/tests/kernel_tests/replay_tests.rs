//! Tests for `AggregateRoot::replay` — streaming rehydration.

use std::fmt;
use std::num::NonZeroUsize;

use nexus::Aggregate;
use nexus::AggregateRoot;
use nexus::AggregateState;
use nexus::DomainEvent;
use nexus::Id;
use nexus::KernelError;
use nexus::Message;
use nexus::Version;

// --- Minimal test domain ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RId(u64);
impl fmt::Display for RId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r-{}", self.0)
    }
}
impl Id for RId {}

#[derive(Debug, Clone, PartialEq)]
enum REvent {
    Added(String),
}
impl Message for REvent {}
impl DomainEvent for REvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Added(_) => "Added",
        }
    }
}

#[derive(Default, Debug, Clone)]
struct RState {
    items: Vec<String>,
}
impl AggregateState for RState {
    type Event = REvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &REvent) -> Self {
        match event {
            REvent::Added(s) => self.items.push(s.clone()),
        }
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("r error")]
struct RErr;

#[derive(Debug)]
struct RAgg;
impl Aggregate for RAgg {
    type State = RState;
    type Error = RErr;
    type Id = RId;
}

// --- Helper: unwrap Version::new ---

const fn v(n: u64) -> Version {
    match Version::new(n) {
        Some(v) => v,
        None => panic!("test version must be non-zero"),
    }
}

// =============================================================================
// 1. replay single event advances version
// =============================================================================

#[test]
fn replay_single_event_advances_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    assert_eq!(agg.version(), None, "fresh aggregate has no version");

    agg.replay(v(1), &REvent::Added("a".into())).unwrap();

    assert_eq!(agg.version(), Some(v(1)));
    assert_eq!(agg.state().items, vec!["a"]);
}

// =============================================================================
// 2. replay sequential events
// =============================================================================

#[test]
fn replay_sequential_events() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();
    agg.replay(v(2), &REvent::Added("b".into())).unwrap();
    agg.replay(v(3), &REvent::Added("c".into())).unwrap();

    assert_eq!(agg.version(), Some(v(3)));
    assert_eq!(agg.state().items, vec!["a", "b", "c"]);
}

// =============================================================================
// 3. replay rejects wrong first version
// =============================================================================

#[test]
fn replay_rejects_wrong_first_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));

    let err = agg.replay(v(5), &REvent::Added("a".into())).unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, v(1), "first replay must expect Version::INITIAL");
            assert_eq!(actual, v(5));
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// 4. replay rejects duplicate version
// =============================================================================

#[test]
fn replay_rejects_duplicate_version() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();

    let err = agg.replay(v(1), &REvent::Added("b".into())).unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, v(2));
            assert_eq!(actual, v(1));
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// 5. replay rejects version gap
// =============================================================================

#[test]
fn replay_rejects_version_gap() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();

    let err = agg.replay(v(3), &REvent::Added("b".into())).unwrap_err();

    match err {
        KernelError::VersionMismatch { expected, actual } => {
            assert_eq!(expected, v(2));
            assert_eq!(actual, v(3));
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

// =============================================================================
// 6. replay does not change version on error
// =============================================================================

#[test]
fn replay_does_not_change_version_on_error() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();

    // Attempt a bad replay — version should remain at 1
    let _ = agg.replay(v(5), &REvent::Added("bad".into()));

    assert_eq!(
        agg.version(),
        Some(v(1)),
        "version must not change on error"
    );
    assert_eq!(
        agg.state().items,
        vec!["a"],
        "state must not change on error"
    );
}

// =============================================================================
// 7. replay enforces rehydration limit
// =============================================================================

#[test]
fn replay_enforces_rehydration_limit() {
    #[derive(Debug)]
    struct SmallLimitAgg;
    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct SmallErr;
    #[allow(clippy::unwrap_used, reason = "3 is non-zero by inspection")]
    impl Aggregate for SmallLimitAgg {
        type State = RState;
        type Error = SmallErr;
        type Id = RId;
        const MAX_REHYDRATION_EVENTS: NonZeroUsize = NonZeroUsize::new(3).unwrap();
    }

    let mut agg = AggregateRoot::<SmallLimitAgg>::new(RId(1));
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();
    agg.replay(v(2), &REvent::Added("b".into())).unwrap();
    agg.replay(v(3), &REvent::Added("c".into())).unwrap();

    // 4th event exceeds the limit of 3
    let err = agg.replay(v(4), &REvent::Added("d".into())).unwrap_err();

    match err {
        KernelError::RehydrationLimitExceeded { max } => {
            assert_eq!(max, 3);
        }
        other => panic!("expected RehydrationLimitExceeded, got {other:?}"),
    }

    // State and version are unchanged after the rejected replay
    assert_eq!(agg.version(), Some(v(3)));
    assert_eq!(agg.state().items, vec!["a", "b", "c"]);
}

// =============================================================================
// 8. replay then advance_version + apply_events continues correctly
// =============================================================================

#[test]
fn replay_then_advance_and_apply_continues_correctly() {
    let mut agg = AggregateRoot::<RAgg>::new(RId(1));

    // Rehydrate from two persisted events
    agg.replay(v(1), &REvent::Added("a".into())).unwrap();
    agg.replay(v(2), &REvent::Added("b".into())).unwrap();
    assert_eq!(agg.version(), Some(v(2)));

    // Simulate: command handler decided two new events, repository persisted them
    let new_events: nexus::Events<_, 1> =
        nexus::events![REvent::Added("c".into()), REvent::Added("d".into())];

    // Repository advances version to reflect persisted events
    agg.advance_version(v(4));
    assert_eq!(agg.version(), Some(v(4)));

    // Repository applies events to keep in-memory state in sync
    agg.apply_events(&new_events);
    assert_eq!(agg.state().items, vec!["a", "b", "c", "d"]);

    // Version stays at what advance_version set — apply_events does not touch it
    assert_eq!(agg.version(), Some(v(4)));
}

// =============================================================================
// 9. replay panic safety — panicking apply preserves original state
// =============================================================================

#[test]
fn replay_panic_safety_preserves_state() {
    // A domain whose apply panics on a specific event
    #[derive(Debug, Clone, PartialEq)]
    enum PanicEvent {
        Normal(String),
        Bomb,
    }
    impl Message for PanicEvent {}
    impl DomainEvent for PanicEvent {
        fn name(&self) -> &'static str {
            match self {
                Self::Normal(_) => "Normal",
                Self::Bomb => "Bomb",
            }
        }
    }

    #[derive(Default, Debug, Clone)]
    struct PanicState {
        items: Vec<String>,
    }
    impl AggregateState for PanicState {
        type Event = PanicEvent;
        fn initial() -> Self {
            Self::default()
        }
        #[allow(clippy::panic, reason = "intentional panic for test")]
        fn apply(mut self, event: &PanicEvent) -> Self {
            match event {
                PanicEvent::Normal(s) => self.items.push(s.clone()),
                PanicEvent::Bomb => panic!("intentional panic in apply"),
            }
            self
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("e")]
    struct PanicErr;

    #[derive(Debug)]
    struct PanicAgg;
    impl Aggregate for PanicAgg {
        type State = PanicState;
        type Error = PanicErr;
        type Id = RId;
    }

    let mut agg = AggregateRoot::<PanicAgg>::new(RId(42));
    agg.replay(v(1), &PanicEvent::Normal("before".into()))
        .unwrap();

    // The Bomb event causes apply to panic — catch it
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agg.replay(v(2), &PanicEvent::Bomb)
    }));

    assert!(result.is_err(), "apply(Bomb) must panic");

    // After the panic, the aggregate must still be in its pre-panic state.
    // This works because AggregateRoot clones state before calling apply,
    // so the original is preserved if apply panics.
    assert_eq!(
        agg.version(),
        Some(v(1)),
        "version must not advance after panic"
    );
    assert_eq!(
        agg.state().items,
        vec!["before"],
        "state must be preserved after panic"
    );

    // The aggregate remains usable — replay can continue from the correct version
    agg.replay(v(2), &PanicEvent::Normal("after".into()))
        .unwrap();
    assert_eq!(agg.version(), Some(v(2)));
    assert_eq!(agg.state().items, vec!["before", "after"]);
}
