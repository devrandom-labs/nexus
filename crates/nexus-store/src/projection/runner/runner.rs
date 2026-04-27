use core::future::{Future, poll_fn};
use core::iter;
use core::pin::pin;
use core::task::Poll;

use nexus::{DomainEvent, Id};

use crate::codec::Codec;
use crate::store::{CheckpointStore, EventStream, Subscription};

use super::error::ProjectionError;
use super::persist::StatePersistence;
use crate::projection::projector::Projector;
use crate::projection::trigger::ProjectionTrigger;

/// Subscription-powered async projection runner.
///
/// Subscribes to an event stream, decodes events, folds them through a
/// [`Projector`], persists state via [`StatePersistence`], and checkpoints
/// progress. Runs until the shutdown signal fires or an error occurs.
///
/// Constructed via [`ProjectionRunner::builder`]. The runner is single-use:
/// `run` consumes `self`. Restart by building a new runner.
pub struct ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
}

impl<I, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Run the projection loop until shutdown or error.
    ///
    /// # Event loop
    ///
    /// 1. Load checkpoint (resume position)
    /// 2. Load persisted state (or use `projector.initial()`)
    /// 3. Subscribe to the event stream from the checkpoint
    /// 4. For each event: decode -> apply -> trigger check -> maybe persist + checkpoint
    /// 5. On shutdown: flush dirty state + checkpoint, return `Ok(())`
    ///
    /// # Errors
    ///
    /// Returns immediately on any error. The supervision layer (separate
    /// component) is responsible for retry/restart policy.
    #[allow(
        clippy::too_many_lines,
        reason = "event loop is a single cohesive unit"
    )]
    pub async fn run(
        self,
        shutdown_signal: impl Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
        } = self;

        // 1. Load checkpoint
        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        // 2. Load persisted state or start fresh
        let (mut state, _loaded_version) = match state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?
        {
            Some((s, v)) => (s, Some(v)),
            None => (projector.initial(), None),
        };

        // Use checkpoint as resume position — it's the source of truth for
        // "which events have been processed."
        let resume_from = last_checkpoint;

        // 3. Subscribe from checkpoint
        let mut stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut shutdown = pin!(shutdown_signal);
        let mut last_persisted_version = last_checkpoint;
        let mut current_version = resume_from;
        let mut dirty = false;

        // 4. Event loop
        loop {
            let step = {
                let mut next = pin!(stream.next());
                poll_fn(|cx| {
                    if shutdown.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Ok(None));
                    }
                    match next.as_mut().poll(cx) {
                        Poll::Ready(Ok(Some(envelope))) => {
                            let version = envelope.version();
                            let decoded =
                                event_codec.decode(envelope.event_type(), envelope.payload());
                            Poll::Ready(
                                decoded
                                    .map(|event| Some((version, event)))
                                    .map_err(ProjectionError::EventCodec),
                            )
                        }
                        Poll::Ready(Ok(None)) => Poll::Ready(Ok(None)),
                        Poll::Ready(Err(e)) => Poll::Ready(Err(ProjectionError::Subscription(e))),
                        Poll::Pending => Poll::Pending,
                    }
                })
                .await?
            };

            let Some((version, event)) = step else {
                // Shutdown or stream ended — flush if dirty
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

            // Apply event
            let event_name = event.name();
            state = projector
                .apply(state, &event)
                .map_err(ProjectionError::Projector)?;
            current_version = Some(version);
            dirty = true;

            // Check trigger
            if trigger.should_project(last_persisted_version, version, iter::once(event_name)) {
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
