use std::iter;

use nexus::{DomainEvent, Id};
use nexus_store::Codec;
use nexus_store::projection::{ProjectionTrigger, Projector};
use nexus_store::store::{CheckpointStore, Subscription};
use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::stream::{DecodeStreamError, DecodedStream};

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
        } = self;

        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let (mut state, _) = match state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?
        {
            Some((s, v)) => (s, Some(v)),
            None => (projector.initial(), None),
        };

        let resume_from = last_checkpoint;

        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut last_persisted_version = last_checkpoint;
        let mut current_version = resume_from;
        let mut dirty = false;

        tokio::pin!(shutdown);

        loop {
            let item = tokio::select! {
                () = &mut shutdown => None,
                item = decoded.next() => item,
            };

            let Some(result) = item else {
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

            let event_name = event.name();
            state = projector
                .apply(state, &event)
                .map_err(ProjectionError::Projector)?;
            current_version = Some(version);
            dirty = true;

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
