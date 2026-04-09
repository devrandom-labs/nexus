//! Formal proptest state machine test for `FjallStore`.
//!
//! Uses `proptest-state-machine` to drive random sequences of operations
//! against the real store while maintaining a `HashMap`-based reference
//! model. Every read is cross-checked against the model, and conflict
//! errors are verified to fire exactly when expected.

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]
#![allow(clippy::panic, reason = "proptest macros use panic")]
#![allow(clippy::missing_panics_doc, reason = "proptest")]
#![allow(clippy::needless_pass_by_value, reason = "proptest")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::items_after_statements, reason = "tests")]
#![allow(clippy::indexing_slicing, reason = "tests")]

use std::collections::HashMap;

use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_store::PendingEnvelope;
use nexus_store::envelope::pending_envelope;
use nexus_store::error::AppendError;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

// ---------------------------------------------------------------------------
// Reference model
// ---------------------------------------------------------------------------

/// Shadow state that mirrors what the real store should contain.
/// Each stream maps to a vector of (version, `event_type`, payload) tuples.
///
/// Event types are always `&'static str` because `PendingEnvelope` requires
/// `'static` event type strings.
/// A single model event: (version, `event_type`, payload).
type ModelEvent = (u64, &'static str, Vec<u8>);

#[derive(Clone, Debug)]
struct ReferenceModel {
    streams: HashMap<String, Vec<ModelEvent>>,
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Transition {
    /// Append `count` events to a stream with the correct expected version.
    AppendValid { stream_name: String, count: usize },
    /// Append with a deliberately wrong expected version (expect conflict).
    AppendConflict { stream_name: String },
    /// Read all events from a stream and verify against the model.
    ReadStream { stream_name: String },
    /// Read from a midpoint version offset (chosen at apply time from model state).
    ReadFromVersion { stream_name: String },
    /// Append to a brand-new stream.
    CreateStream { stream_name: String },
    /// Append an empty batch (should succeed without changing version).
    AppendEmptyBatch { stream_name: String },
}

// ---------------------------------------------------------------------------
// Stream-name pool (small to encourage collisions)
// ---------------------------------------------------------------------------

const STREAM_POOL: &[&str] = &["stream-a", "stream-b", "stream-c", "stream-d", "stream-e"];

fn stream_name_strategy() -> BoxedStrategy<String> {
    prop::sample::select(STREAM_POOL)
        .prop_map(std::string::ToString::to_string)
        .boxed()
}

fn existing_stream_strategy(state: &ReferenceModel) -> BoxedStrategy<String> {
    let keys: Vec<String> = state.streams.keys().cloned().collect();
    prop::sample::select(keys).boxed()
}

// ---------------------------------------------------------------------------
// ReferenceStateMachine impl
// ---------------------------------------------------------------------------

impl ReferenceStateMachine for ReferenceModel {
    type State = Self;
    type Transition = Transition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(Self {
            streams: HashMap::new(),
        })
        .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        let has_streams = !state.streams.is_empty();

        // Strategies that always apply (don't need existing streams).
        let create =
            stream_name_strategy().prop_map(|stream_name| Transition::CreateStream { stream_name });

        if has_streams {
            let append_valid = (existing_stream_strategy(state), 1..5usize)
                .prop_map(|(stream_name, count)| Transition::AppendValid { stream_name, count });
            let append_conflict = existing_stream_strategy(state)
                .prop_map(|stream_name| Transition::AppendConflict { stream_name });
            let read_stream = existing_stream_strategy(state)
                .prop_map(|stream_name| Transition::ReadStream { stream_name });
            let read_from_version = existing_stream_strategy(state)
                .prop_map(move |stream_name| Transition::ReadFromVersion { stream_name });
            let append_empty = existing_stream_strategy(state)
                .prop_map(|stream_name| Transition::AppendEmptyBatch { stream_name });

            prop_oneof![
                3 => append_valid,
                1 => append_conflict,
                2 => read_stream,
                2 => read_from_version,
                2 => create,
                1 => append_empty,
            ]
            .boxed()
        } else {
            // No streams exist yet - only creation is possible.
            create.boxed()
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            Transition::CreateStream { stream_name } => {
                // Idempotent: only insert if the stream doesn't already exist.
                state
                    .streams
                    .entry(stream_name.clone())
                    .or_insert_with(Vec::new);
            }
            Transition::AppendValid { stream_name, count } => {
                let events = state
                    .streams
                    .entry(stream_name.clone())
                    .or_insert_with(Vec::new);
                let current_version = events.last().map_or(0, |(v, _, _)| *v);
                for i in 1..=*count {
                    let version = current_version + i as u64;
                    let payload = format!("payload-{stream_name}-{version}").into_bytes();
                    events.push((version, "TestEvent", payload));
                }
            }
            Transition::AppendConflict { .. }
            | Transition::ReadStream { .. }
            | Transition::ReadFromVersion { .. }
            | Transition::AppendEmptyBatch { .. } => {
                // These transitions don't modify the reference model.
            }
        }
        state
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            Transition::AppendConflict { stream_name } => {
                // Must have an existing stream to conflict with.
                state.streams.contains_key(stream_name)
            }
            Transition::ReadStream { stream_name }
            | Transition::ReadFromVersion { stream_name } => {
                state.streams.contains_key(stream_name)
            }
            Transition::AppendEmptyBatch { stream_name } => {
                // Allow on both existing and non-existing streams. However,
                // since existing_stream_strategy picks from keys, if it
                // selects a name that was later removed by shrinking, skip it.
                // For simplicity, we allow all names.
                let _ = stream_name;
                true
            }
            Transition::CreateStream { .. } | Transition::AppendValid { .. } => true,
        }
    }
}

// ---------------------------------------------------------------------------
// System Under Test (SUT)
// ---------------------------------------------------------------------------

struct FjallSut {
    store: FjallStore,
    /// Keep the tempdir alive so the database isn't deleted.
    _dir: tempfile::TempDir,
    /// Tokio runtime for running async store operations.
    rt: tokio::runtime::Runtime,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);
impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl nexus::Id for TestId {}
fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

fn make_envelope(version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope<()> {
    pending_envelope(Version::new(version).unwrap())
        .event_type(event_type)
        .payload(payload.to_vec())
        .build_without_metadata()
}

// ---------------------------------------------------------------------------
// StateMachineTest impl
// ---------------------------------------------------------------------------

struct FjallStateMachineTest;

impl StateMachineTest for FjallStateMachineTest {
    type SystemUnderTest = FjallSut;
    type Reference = ReferenceModel;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        FjallSut {
            store,
            _dir: dir,
            rt,
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "state machine apply covers all transitions"
    )]
    fn apply(
        sut: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        match transition {
            Transition::CreateStream { ref stream_name } => {
                let stream_id = tid(stream_name);
                let model_events = ref_state.streams.get(stream_name);
                let model_len = model_events.map_or(0, Vec::len);

                // CreateStream in the model just ensures the entry exists.
                // If the model has events, the stream was already created by
                // prior AppendValid transitions. If it has 0 events, we just
                // do nothing (the store auto-creates on first real append).
                // Verify we can read the stream (empty or with data).
                let result = sut
                    .rt
                    .block_on(sut.store.read_stream(&stream_id, Version::INITIAL));
                let mut stream = result.unwrap();
                let mut count = 0usize;
                while let Some(item) = sut.rt.block_on(stream.next()) {
                    let _ = item.unwrap();
                    count += 1;
                }
                assert_eq!(
                    count, model_len,
                    "CreateStream: stream '{stream_name}' event count mismatch"
                );
            }

            Transition::AppendValid {
                ref stream_name,
                count,
            } => {
                let stream_id = tid(stream_name);
                let model_events = &ref_state.streams[stream_name];

                // The model already has the new events appended. The last
                // `count` entries are the ones we need to write.
                let new_events = &model_events[model_events.len() - count..];
                let expected_version = if model_events.len() == count {
                    // All events in the stream are new - this is the first append.
                    None
                } else {
                    // There were prior events; expected = version just before new.
                    let prior_version = model_events[model_events.len() - count - 1].0;
                    Version::new(prior_version)
                };

                let envelopes: Vec<PendingEnvelope<()>> = new_events
                    .iter()
                    .map(|(v, et, pl)| make_envelope(*v, et, pl))
                    .collect();

                let result =
                    sut.rt
                        .block_on(sut.store.append(&stream_id, expected_version, &envelopes));
                assert!(
                    result.is_ok(),
                    "AppendValid to '{stream_name}' failed: {:?}",
                    result.unwrap_err()
                );

                // Verify all events in the stream match the model.
                verify_stream(&sut, stream_name, model_events);
            }

            Transition::AppendConflict { ref stream_name } => {
                let stream_id = tid(stream_name);
                let model_events = &ref_state.streams[stream_name];
                let current_version = model_events.last().map_or(0, |(v, _, _)| *v);

                // Pick a wrong expected version: if current is 0, use 99;
                // otherwise use None (definitely wrong since stream has events).
                let wrong_version: Option<Version> = if current_version == 0 {
                    Version::new(99)
                } else {
                    None
                };

                // Build a single envelope with version = wrong + 1.
                let env_version = wrong_version.map_or(1, |v| v.as_u64() + 1);
                let envelope = make_envelope(env_version, "Conflict", b"should-fail");

                let result =
                    sut.rt
                        .block_on(sut.store.append(&stream_id, wrong_version, &[envelope]));
                assert!(
                    result.is_err(),
                    "AppendConflict to '{stream_name}' should have failed but succeeded"
                );
                match result.unwrap_err() {
                    AppendError::Conflict { .. } => {
                        // Expected error.
                    }
                    other @ AppendError::Store(_) => {
                        panic!("AppendConflict: expected Conflict error, got: {other}")
                    }
                }
            }

            Transition::ReadStream { ref stream_name } => {
                let model_events = &ref_state.streams[stream_name];
                verify_stream(&sut, stream_name, model_events);
            }

            Transition::ReadFromVersion { ref stream_name } => {
                let model_events = &ref_state.streams[stream_name];
                if model_events.is_empty() {
                    // Empty stream: reading from version 1 yields nothing.
                    let stream_id = tid(stream_name);
                    let result = sut
                        .rt
                        .block_on(sut.store.read_stream(&stream_id, Version::INITIAL));
                    let mut stream = result.unwrap();
                    assert!(
                        sut.rt.block_on(stream.next()).is_none(),
                        "ReadFromVersion: empty stream should yield no events"
                    );
                } else {
                    // Pick the midpoint version to read from.
                    let mid_idx = model_events.len() / 2;
                    let from_version = model_events[mid_idx].0;
                    let expected: Vec<&(u64, &'static str, Vec<u8>)> = model_events
                        .iter()
                        .filter(|(v, _, _)| *v >= from_version)
                        .collect();

                    let stream_id = tid(stream_name);
                    let result = sut.rt.block_on(
                        sut.store
                            .read_stream(&stream_id, Version::new(from_version).unwrap()),
                    );
                    let mut stream = result.unwrap();

                    for entry in &expected {
                        let (v, et, pl) = *entry;
                        let item = sut
                            .rt
                            .block_on(stream.next())
                            .unwrap_or_else(|| {
                                panic!(
                                    "ReadFromVersion '{stream_name}' from {from_version}: \
                                     expected event at version {v} but stream ended"
                                )
                            })
                            .unwrap();
                        assert_eq!(
                            item.version(),
                            Version::new(*v).unwrap(),
                            "ReadFromVersion: version mismatch"
                        );
                        assert_eq!(
                            item.event_type(),
                            *et,
                            "ReadFromVersion: event_type mismatch at version {v}"
                        );
                        assert_eq!(
                            item.payload(),
                            pl.as_slice(),
                            "ReadFromVersion: payload mismatch at version {v}"
                        );
                    }
                    assert!(
                        sut.rt.block_on(stream.next()).is_none(),
                        "ReadFromVersion: unexpected trailing events"
                    );
                }
            }

            Transition::AppendEmptyBatch { ref stream_name } => {
                let stream_id = tid(stream_name);
                let model_events = ref_state.streams.get(stream_name);
                let current_version = model_events
                    .and_then(|evts| evts.last())
                    .map_or(0, |(v, _, _)| *v);

                let expected = Version::new(current_version);
                let result = sut.rt.block_on(sut.store.append(&stream_id, expected, &[]));
                assert!(
                    result.is_ok(),
                    "AppendEmptyBatch to '{stream_name}' failed: {:?}",
                    result.unwrap_err()
                );
            }
        }

        sut
    }
}

/// Helper: read all events from a stream and verify they match the model.
fn verify_stream(sut: &FjallSut, stream_name: &str, model_events: &[ModelEvent]) {
    let stream_id = tid(stream_name);
    let result = sut
        .rt
        .block_on(sut.store.read_stream(&stream_id, Version::INITIAL));
    let mut stream = result.unwrap();

    for (v, et, pl) in model_events {
        let item = sut
            .rt
            .block_on(stream.next())
            .unwrap_or_else(|| {
                panic!(
                    "verify_stream '{stream_name}': expected event at version {v} \
                     but stream ended early"
                )
            })
            .unwrap();
        assert_eq!(
            item.version(),
            Version::new(*v).unwrap(),
            "verify_stream '{stream_name}': version mismatch"
        );
        assert_eq!(
            item.event_type(),
            *et,
            "verify_stream '{stream_name}': event_type mismatch at version {v}"
        );
        assert_eq!(
            item.payload(),
            pl.as_slice(),
            "verify_stream '{stream_name}': payload mismatch at version {v}"
        );
    }
    assert!(
        sut.rt.block_on(stream.next()).is_none(),
        "verify_stream '{stream_name}': unexpected trailing events \
         (model has {} events)",
        model_events.len()
    );
}

// ---------------------------------------------------------------------------
// Test entry point
// ---------------------------------------------------------------------------

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 64,
        .. proptest::test_runner::Config::default()
    })]

    #[test]
    fn fjall_store_state_machine(sequential 1..50 => FjallStateMachineTest);
}
