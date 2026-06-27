use nexus::*;
use std::fmt;

// --- Test ID type ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

// --- Test Events ---
#[derive(Debug, Clone)]
struct ThingCreated {
    name: String,
}

#[derive(Debug, Clone)]
struct ThingActivated;

#[derive(Debug, Clone)]
enum ThingEvent {
    Created(ThingCreated),
    Activated(ThingActivated),
}

impl Message for ThingEvent {}
impl DomainEvent for ThingEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "ThingCreated",
            Self::Activated(_) => "ThingActivated",
        }
    }
}

// --- Test State ---
#[derive(Default, Debug, Clone)]
struct ThingState {
    name: String,
    active: bool,
}

impl AggregateState for ThingState {
    type Event = ThingEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &ThingEvent) -> Self {
        match event {
            ThingEvent::Created(e) => self.name.clone_from(&e.name),
            ThingEvent::Activated(_) => self.active = true,
        }
        self
    }
}

// --- Test Aggregate ---
struct ThingAggregate;

#[derive(Debug, thiserror::Error)]
#[allow(
    dead_code,
    reason = "test-only error type; variants used for type checking"
)]
enum ThingError {
    #[error("already exists")]
    AlreadyExists,
}

impl Aggregate for ThingAggregate {
    type State = ThingState;
    type Error = ThingError;
    type Id = TestId;
}

// --- Tests ---
#[test]
fn event_of_resolves_correctly() {
    fn assert_event_type<A: Aggregate>()
    where
        EventOf<A>: DomainEvent,
    {
    }
    assert_event_type::<ThingAggregate>();
}

#[test]
fn domain_event_name_works() {
    let event = ThingEvent::Created(ThingCreated {
        name: "test".into(),
    });
    assert_eq!(event.name(), "ThingCreated");
    let event = ThingEvent::Activated(ThingActivated);
    assert_eq!(event.name(), "ThingActivated");
}

#[test]
fn aggregate_state_apply_mutates() {
    let state = ThingState::default();
    let state = state.apply(&ThingEvent::Created(ThingCreated {
        name: "hello".into(),
    }));
    assert_eq!(state.name, "hello");
    assert!(!state.active);
    let state = state.apply(&ThingEvent::Activated(ThingActivated));
    assert!(state.active);
}

#[test]
fn id_to_label_returns_errorid_verbatim_for_short_id() {
    // `to_label` now returns the sealed `ErrorId`, not `arrayvec::ArrayString`.
    let id = TestId("order-123".into());
    let label: ErrorId = id.to_label();
    assert_eq!(label.as_str(), "order-123");
}

#[test]
fn id_to_label_signals_truncation_with_ellipsis() {
    // An ID whose Display exceeds 64 bytes is truncated, and — unlike the old
    // silent `LabelWriter` — the overflow is *signalled* with a trailing `…`.
    let long_name = "x".repeat(100);
    let id = TestId(long_name);
    let label = id.to_label();
    assert!(label.len() <= 64, "label must fit the 64-byte cap");
    assert!(
        label.as_str().ends_with('…'),
        "truncation must be visually signalled with `…`, got {:?}",
        label.as_str()
    );
    // The verbatim prefix is preserved up to the cap (minus the marker).
    assert!(label.as_str().starts_with("xxxx"));
}

#[test]
fn id_to_label_truncation_is_utf8_safe_on_multibyte_boundary() {
    // Defensive boundary (CLAUDE.md rule 7): a multi-byte char straddling the
    // cap must not panic and must leave the label valid UTF-8. `あ` is 3 bytes,
    // so the 64-byte cap lands mid-character and forces a boundary backstep.
    let long_name = "あ".repeat(40); // 120 bytes
    let id = TestId(long_name);
    let label = id.to_label();
    assert!(label.len() <= 64, "label must fit the 64-byte cap");
    assert!(
        label.as_str().ends_with('…'),
        "truncation must be signalled with `…`, got {:?}",
        label.as_str()
    );
    // Every retained char is the intact `あ` (no split code unit) plus the `…`.
    assert!(
        label.as_str().chars().all(|c| c == 'あ' || c == '…'),
        "truncation left a broken/unexpected char: {:?}",
        label.as_str()
    );
}
