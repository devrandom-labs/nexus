use thiserror::Error;

/// Errors from the projection runner.
///
/// Generic over projector (`P`), event codec (`EC`), state persistence (`SP`),
/// checkpoint store (`Ckpt`), and subscription (`Sub`) error types.
/// When state persistence is disabled (`NoStatePersistence`), `SP = Infallible`
/// and the `State` variant is unconstructable.
#[derive(Debug, Error)]
pub enum ProjectionError<P, EC, SP, Ckpt, Sub> {
    /// Projector `apply` failed (business logic error).
    #[error("projector apply failed: {0}")]
    Projector(#[source] P),

    /// Event deserialization failed.
    #[error("event codec failed: {0}")]
    EventCodec(#[source] EC),

    /// State persistence failed (store or codec).
    #[error("state persistence failed: {0}")]
    State(#[source] SP),

    /// Checkpoint load or save failed.
    #[error("checkpoint failed: {0}")]
    Checkpoint(#[source] Ckpt),

    /// Subscription or event stream failed.
    #[error("subscription failed: {0}")]
    Subscription(#[source] Sub),
}

/// Errors from state persistence operations.
///
/// Wraps both state store and state codec errors. Used as
/// `SP::Error` in [`ProjectionError`] when state persistence is enabled.
#[derive(Debug, Error)]
pub enum StatePersistError<S, C> {
    /// State store operation failed (load or save).
    #[error("state store error: {0}")]
    Store(#[source] S),

    /// State codec operation failed (encode or decode).
    #[error("state codec error: {0}")]
    Codec(#[source] C),
}
