use nexus::kernel::aggregate::AggregateRoot;
use nexus::kernel::*;
use std::fmt;

// --- Self-contained test domain ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for TestId {}

#[derive(Debug, Clone)]
struct ItemCreated {
    name: String,
}
#[derive(Debug, Clone)]
struct ItemDone;

#[derive(Debug, Clone)]
enum ItemEvent {
    Created(ItemCreated),
    Done(ItemDone),
}
impl Message for ItemEvent {}
impl DomainEvent for ItemEvent {
    fn name(&self) -> &'static str {
        match self {
            ItemEvent::Created(_) => "ItemCreated",
            ItemEvent::Done(_) => "ItemDone",
        }
    }
}

#[derive(Default, Debug)]
struct ItemState {
    name: String,
    done: bool,
}
impl AggregateState for ItemState {
    type Event = ItemEvent;
    fn apply(&mut self, event: &ItemEvent) {
        match event {
            ItemEvent::Created(e) => self.name = e.name.clone(),
            ItemEvent::Done(_) => self.done = true,
        }
    }
    fn name(&self) -> &'static str {
        "Item"
    }
}

#[derive(Debug)]
struct ItemAggregate;
#[derive(Debug, thiserror::Error)]
enum ItemError {
    #[error("already exists")]
    AlreadyExists,
    #[error("already done")]
    AlreadyDone,
}
impl Aggregate for ItemAggregate {
    type State = ItemState;
    type Error = ItemError;
    type Id = TestId;
}

// --- Business logic helpers (orphan rules prevent impl on AggregateRoot in tests) ---
fn create_item(agg: &mut AggregateRoot<ItemAggregate>, name: String) -> Result<(), ItemError> {
    if !agg.state().name.is_empty() {
        return Err(ItemError::AlreadyExists);
    }
    agg.apply_event(ItemEvent::Created(ItemCreated { name }));
    Ok(())
}

fn mark_item_done(agg: &mut AggregateRoot<ItemAggregate>) -> Result<(), ItemError> {
    if agg.state().done {
        return Err(ItemError::AlreadyDone);
    }
    agg.apply_event(ItemEvent::Done(ItemDone));
    Ok(())
}

// --- Tests ---
#[test]
fn new_aggregate_has_initial_version() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    assert_eq!(agg.version(), Version::INITIAL);
    assert_eq!(agg.current_version(), Version::INITIAL);
}

#[test]
fn new_aggregate_has_default_state() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    assert_eq!(agg.state().name, "");
    assert!(!agg.state().done);
}

#[test]
fn apply_event_mutates_state_and_tracks_uncommitted() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.apply_event(ItemEvent::Created(ItemCreated {
        name: "test".into(),
    }));
    assert_eq!(agg.state().name, "test");
    assert_eq!(agg.current_version(), Version::from(1u64));
    assert_eq!(agg.version(), Version::INITIAL); // persisted version unchanged
}

#[test]
fn take_uncommitted_events_drains() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    agg.apply_event(ItemEvent::Created(ItemCreated {
        name: "test".into(),
    }));
    agg.apply_event(ItemEvent::Done(ItemDone));

    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].version, Version::from(1u64));
    assert_eq!(events[1].version, Version::from(2u64));

    let events2 = agg.take_uncommitted_events();
    assert!(events2.is_empty());
}

#[test]
fn load_from_events_rehydrates() {
    let events = vec![
        VersionedEvent {
            version: Version::from(1u64),
            event: ItemEvent::Created(ItemCreated {
                name: "loaded".into(),
            }),
        },
        VersionedEvent {
            version: Version::from(2u64),
            event: ItemEvent::Done(ItemDone),
        },
    ];
    let agg = AggregateRoot::<ItemAggregate>::load_from_events(TestId("1".into()), events).unwrap();
    assert_eq!(agg.version(), Version::from(2u64));
    assert_eq!(agg.state().name, "loaded");
    assert!(agg.state().done);
}

#[test]
fn load_from_events_rejects_version_gap() {
    let events = vec![
        VersionedEvent {
            version: Version::from(1u64),
            event: ItemEvent::Created(ItemCreated {
                name: "test".into(),
            }),
        },
        VersionedEvent {
            version: Version::from(3u64),
            event: ItemEvent::Done(ItemDone),
        },
    ];
    let result = AggregateRoot::<ItemAggregate>::load_from_events(TestId("1".into()), events);
    assert!(result.is_err());
    match result.unwrap_err() {
        KernelError::VersionMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, Version::from(2u64));
            assert_eq!(actual, Version::from(3u64));
        }
    }
}

#[test]
fn business_logic_on_aggregate_works() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    create_item(&mut agg, "My Item".into()).unwrap();
    assert_eq!(agg.state().name, "My Item");
    mark_item_done(&mut agg).unwrap();
    assert!(agg.state().done);
    let events = agg.take_uncommitted_events();
    assert_eq!(events.len(), 2);
}

#[test]
fn business_logic_enforces_invariants() {
    let mut agg = AggregateRoot::<ItemAggregate>::new(TestId("1".into()));
    create_item(&mut agg, "Item".into()).unwrap();
    let err = create_item(&mut agg, "Another".into()).unwrap_err();
    assert!(matches!(err, ItemError::AlreadyExists));
}

#[test]
fn aggregate_id_accessible() {
    let agg = AggregateRoot::<ItemAggregate>::new(TestId("abc".into()));
    assert_eq!(agg.id(), &TestId("abc".into()));
}
