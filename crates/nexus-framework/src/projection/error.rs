use thiserror::Error;

/// Errors from the projection runner.
///
/// Generic over projector (`P`), event codec (`EC`), state store (`SP`),
/// checkpoint store (`Ckpt`), and subscription (`Sub`) error types.
/// When state persistence is disabled (`SP = ()`), `SP = Infallible`
/// and the `State` variant is unconstructable.
#[derive(Debug, Error)]
pub enum ProjectionError<P, EC, SP, Ckpt, Sub> {
    /// Projector `apply` failed (business logic error).
    #[error("projector apply failed: {0}")]
    Projector(#[source] P),

    /// Event deserialization failed.
    #[error("event codec failed: {0}")]
    EventCodec(#[source] EC),

    /// State store failed (load, save, or delete).
    #[error("state store failed: {0}")]
    State(#[source] SP),

    /// Checkpoint load or save failed.
    #[error("checkpoint failed: {0}")]
    Checkpoint(#[source] Ckpt),

    /// Subscription or event stream failed.
    #[error("subscription failed: {0}")]
    Subscription(#[source] Sub),
}

/// `try_fold_async_until` requires `E: From<Self::Error>` where
/// `Self::Error` is the subscription stream's error type. Since
/// [`Subscription::Stream<'a>::Error == Subscription::Error`] in
/// [`nexus_store::store::Subscription`], routing that error into
/// [`ProjectionError::Subscription`] gives the combinator what it
/// needs without extra glue at the call site.
impl<P, EC, SP, Ckpt, Sub> From<Sub> for ProjectionError<P, EC, SP, Ckpt, Sub> {
    fn from(err: Sub) -> Self {
        Self::Subscription(err)
    }
}
