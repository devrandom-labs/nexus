use std::iter;
use std::marker::PhantomData;

use nexus::{DomainEvent, Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::stream::{DecodeStreamError, DecodedStream};

// ═══════════════════════════════════════════════════════════════════════════
// Typestate markers — encode the startup decision at the type level
// ═══════════════════════════════════════════════════════════════════════════

/// Startup decision: resuming from a checkpoint with loaded state.
pub struct Resuming;

/// Startup decision: schema mismatch detected, replaying from beginning.
pub struct Rebuilding;

/// Startup decision: first run, processing from beginning.
pub struct Starting;

// ═══════════════════════════════════════════════════════════════════════════
// Sync core — pure decision functions, no IO
// ═══════════════════════════════════════════════════════════════════════════

/// Process a single decoded event: fold through the projector, evaluate trigger.
///
/// Returns the new state and whether the trigger fired (indicating the
/// caller should persist state + checkpoint).
#[allow(
    clippy::too_many_arguments,
    reason = "pure function — each argument carries distinct domain information"
)]
fn apply_event<P, E, Trig>(
    projector: &P,
    trigger: &Trig,
    state: P::State,
    event: &E,
    last_persisted_version: Option<Version>,
    version: Version,
) -> Result<(P::State, bool), P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    let event_name = event.name();
    let new_state = projector.apply(state, event)?;
    let should_persist =
        trigger.should_project(last_persisted_version, version, iter::once(event_name));
    Ok((new_state, should_persist))
}

// ═══════════════════════════════════════════════════════════════════════════
// PreparedProjection
// ═══════════════════════════════════════════════════════════════════════════

/// A projection that has completed startup and is ready to run.
///
/// Created by [`ProjectionRunner::initialize`](super::ProjectionRunner).
/// The `Mode` parameter encodes the startup decision as a typestate:
/// - [`Resuming`] — loaded state, will resume from checkpoint
/// - [`Rebuilding`] — schema mismatch, will replay from beginning
/// - [`Starting`] — first run, will process from beginning
///
/// Call [`run`](PreparedProjection::run) to enter the event loop, or inspect
/// the resolved state via mode-specific accessors before running.
pub struct PreparedProjection<I, Sub, Ckpt, SP, P: Projector, EC, Trig, Mode> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
    pub(crate) state: P::State,
    pub(crate) resume_from: Option<Version>,
    pub(crate) _mode: PhantomData<Mode>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Async shell — IO orchestration only
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, Ckpt, SP, P, EC, Trig, Mode> PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Mode>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Run the event loop until shutdown or error.
    ///
    /// 1. Subscribe to the event stream from the resolved resume position
    /// 2. For each event: decode -> apply -> trigger check -> maybe persist + checkpoint
    /// 3. On shutdown: flush dirty state + checkpoint, return `Ok(())`
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] on subscription, codec, projector,
    /// state persistence, or checkpoint failure.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
            mut state,
            resume_from,
            _mode: _,
        } = self;

        // ── IO: subscribe ─────────────────────────────────────────────
        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut last_persisted_version = resume_from;
        let mut current_version = resume_from;
        let mut dirty = false;

        tokio::pin!(shutdown);

        loop {
            let item = tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            };

            let Some(result) = item else {
                // ── IO: flush on shutdown ──────────────────────────────
                if let (true, Some(ver)) = (dirty, current_version) {
                    state_persistence
                        .save(&id, ver, &state)
                        .await
                        .map_err(ProjectionError::State)?;
                    checkpoint
                        .save(&id, ver)
                        .await
                        .map_err(ProjectionError::Checkpoint)?;
                }
                return Ok(());
            };

            let (version, event) = match result {
                Ok(pair) => pair,
                Err(DecodeStreamError::Stream(e)) => {
                    return Err(ProjectionError::Subscription(e));
                }
                Err(DecodeStreamError::Codec(e)) => {
                    return Err(ProjectionError::EventCodec(e));
                }
            };

            // ── Sync: fold + trigger ──────────────────────────────────
            let (new_state, should_persist) = apply_event(
                &projector,
                &trigger,
                state,
                &event,
                last_persisted_version,
                version,
            )
            .map_err(ProjectionError::Projector)?;

            state = new_state;
            current_version = Some(version);
            dirty = true;

            // ── IO: persist if triggered ──────────────────────────────
            if should_persist {
                state_persistence
                    .save(&id, version, &state)
                    .await
                    .map_err(ProjectionError::State)?;
                checkpoint
                    .save(&id, version)
                    .await
                    .map_err(ProjectionError::Checkpoint)?;
                last_persisted_version = Some(version);
                dirty = false;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Typestate-specific methods
// ═══════════════════════════════════════════════════════════════════════════

impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>
{
    /// The resolved resume position from the checkpoint.
    #[must_use]
    pub const fn resume_from(&self) -> Option<Version> {
        self.resume_from
    }

    /// The loaded projection state.
    #[must_use]
    pub const fn state(&self) -> &P::State {
        &self.state
    }

    /// Discard loaded state and force a full rebuild from the beginning.
    ///
    /// Transitions the typestate from [`Resuming`] to [`Rebuilding`].
    /// The state is reset to [`Projector::initial()`] and the resume
    /// position is set to `None` (start of stream).
    #[must_use]
    pub fn force_rebuild(self) -> PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding> {
        let initial_state = self.projector.initial();
        PreparedProjection {
            id: self.id,
            subscription: self.subscription,
            checkpoint: self.checkpoint,
            state_persistence: self.state_persistence,
            projector: self.projector,
            event_codec: self.event_codec,
            trigger: self.trigger,
            state: initial_state,
            resume_from: None,
            _mode: PhantomData,
        }
    }
}

impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>
{
    /// The initial projection state (always [`Projector::initial()`]).
    #[must_use]
    pub const fn state(&self) -> &P::State {
        &self.state
    }
}

impl<I, Sub, Ckpt, SP, P: Projector, EC, Trig>
    PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>
{
    /// The initial projection state (always [`Projector::initial()`]).
    #[must_use]
    pub const fn state(&self) -> &P::State {
        &self.state
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code — relaxed lints"
)]
mod tests {
    use std::convert::Infallible;
    use std::num::NonZeroU64;

    use nexus::{DomainEvent, Message, Version};
    use nexus_store::projection::{EveryNEvents, ProjectionTrigger, Projector};

    use super::apply_event;
    use super::*;

    // ── Minimal test fixtures ─────────────────────────────────────────

    #[derive(Debug)]
    struct Evt;
    impl Message for Evt {}
    impl DomainEvent for Evt {
        fn name(&self) -> &'static str {
            "Evt"
        }
    }

    struct SumProjector;
    impl Projector for SumProjector {
        type Event = Evt;
        type State = u64;
        type Error = Infallible;

        fn initial(&self) -> u64 {
            0
        }

        fn apply(&self, state: u64, _event: &Evt) -> Result<u64, Infallible> {
            Ok(state.wrapping_add(1))
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("apply failed")]
    struct ApplyError;

    struct FailingProjector;
    impl Projector for FailingProjector {
        type Event = Evt;
        type State = u64;
        type Error = ApplyError;

        fn initial(&self) -> u64 {
            0
        }

        fn apply(&self, _state: u64, _event: &Evt) -> Result<u64, ApplyError> {
            Err(ApplyError)
        }
    }

    struct AlwaysTrigger;
    impl ProjectionTrigger for AlwaysTrigger {
        fn should_project(
            &self,
            _old: Option<Version>,
            _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool {
            true
        }
    }

    struct NeverTrigger;
    impl ProjectionTrigger for NeverTrigger {
        fn should_project(
            &self,
            _old: Option<Version>,
            _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool {
            false
        }
    }

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    // ── apply_event tests ─────────────────────────────────────────────

    #[test]
    fn apply_event_folds_state_through_projector() {
        let (new_state, _) =
            apply_event(&SumProjector, &NeverTrigger, 5_u64, &Evt, None, v(1)).unwrap();
        assert_eq!(new_state, 6);
    }

    #[test]
    fn apply_event_returns_should_persist_true_when_trigger_fires() {
        let (_, should_persist) =
            apply_event(&SumProjector, &AlwaysTrigger, 0_u64, &Evt, None, v(1)).unwrap();
        assert!(should_persist);
    }

    #[test]
    fn apply_event_returns_should_persist_false_when_trigger_does_not_fire() {
        let (_, should_persist) =
            apply_event(&SumProjector, &NeverTrigger, 0_u64, &Evt, None, v(1)).unwrap();
        assert!(!should_persist);
    }

    #[test]
    fn apply_event_propagates_projector_error() {
        let result = apply_event(&FailingProjector, &AlwaysTrigger, 0_u64, &Evt, None, v(1));
        assert!(result.is_err());
    }

    #[test]
    fn apply_event_passes_versions_to_trigger() {
        let every_3 = EveryNEvents(NonZeroU64::new(3).unwrap());

        let (_, should_persist) =
            apply_event(&SumProjector, &every_3, 0_u64, &Evt, Some(v(2)), v(3)).unwrap();
        assert!(should_persist);

        let (_, should_persist) =
            apply_event(&SumProjector, &every_3, 0_u64, &Evt, Some(v(3)), v(4)).unwrap();
        assert!(!should_persist);
    }

    // ── PreparedProjection typestate tests ─────────────────────────────

    fn make_resuming_prepared()
    -> PreparedProjection<&'static str, (), (), (), SumProjector, (), NeverTrigger, Resuming> {
        PreparedProjection {
            id: "test",
            subscription: (),
            checkpoint: (),
            state_persistence: (),
            projector: SumProjector,
            event_codec: (),
            trigger: NeverTrigger,
            state: 42,
            resume_from: Some(v(5)),
            _mode: PhantomData,
        }
    }

    #[test]
    fn resuming_state_returns_loaded_state() {
        let p = make_resuming_prepared();
        assert_eq!(*p.state(), 42);
    }

    #[test]
    fn resuming_resume_from_returns_checkpoint() {
        let p = make_resuming_prepared();
        assert_eq!(p.resume_from(), Some(v(5)));
    }

    #[test]
    fn force_rebuild_transitions_to_rebuilding_with_initial_state() {
        let p = make_resuming_prepared();
        let rebuilt = p.force_rebuild();
        // State reset to projector.initial() = 0
        assert_eq!(*rebuilt.state(), 0);
    }

    #[test]
    fn force_rebuild_sets_resume_from_to_none() {
        let p = make_resuming_prepared();
        let rebuilt = p.force_rebuild();
        // resume_from should be None (replay from beginning)
        // Access via pub(crate) field since we're in the same crate
        assert!(rebuilt.resume_from.is_none());
    }
}
