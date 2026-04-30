use std::marker::PhantomData;

use nexus::{Id, Version};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::status::{ProjectionStatus, apply_event};
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
            state,
            resume_from,
            _mode: _,
        } = self;

        // ── IO: subscribe ─────────────────────────────────────────────
        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut status = ProjectionStatus::Idle {
            state,
            checkpoint: resume_from,
        };

        tokio::pin!(shutdown);

        loop {
            match tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            } {
                None => {
                    // ── IO: flush on shutdown ─────────────────────────
                    if let ProjectionStatus::Pending {
                        ref state, version, ..
                    } = status
                    {
                        state_persistence
                            .save(&id, version, state)
                            .await
                            .map_err(ProjectionError::State)?;
                        checkpoint
                            .save(&id, version)
                            .await
                            .map_err(ProjectionError::Checkpoint)?;
                    }
                    return Ok(());
                }
                Some(Ok((version, event))) => {
                    status = apply_event(&projector, &trigger, status, &event, version)
                        .map_err(ProjectionError::Projector)?;

                    // ── IO: persist if triggered ─────────────────────
                    if let ProjectionStatus::Committed {
                        ref state, version, ..
                    } = status
                    {
                        state_persistence
                            .save(&id, version, state)
                            .await
                            .map_err(ProjectionError::State)?;
                        checkpoint
                            .save(&id, version)
                            .await
                            .map_err(ProjectionError::Checkpoint)?;
                    }
                }
                Some(Err(DecodeStreamError::Stream(e))) => {
                    return Err(ProjectionError::Subscription(e));
                }
                Some(Err(DecodeStreamError::Codec(e))) => {
                    return Err(ProjectionError::EventCodec(e));
                }
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

    use nexus::{DomainEvent, Message, Version};
    use nexus_store::projection::{ProjectionTrigger, Projector};

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
