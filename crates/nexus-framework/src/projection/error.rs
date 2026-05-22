use thiserror::Error;

/// Errors from the projection runner.
///
/// Generic over projector (`P`), event codec (`EC`), snapshot store (`SS`),
/// and subscription (`Sub`) error types.
#[derive(Debug, Error)]
pub enum ProjectionError<P, EC, SS, Sub> {
    /// Projector `apply` failed (business logic error).
    #[error("projector apply failed: {0}")]
    Projector(#[source] P),

    /// Event deserialization failed.
    #[error("event codec failed: {0}")]
    EventCodec(#[source] EC),

    /// Snapshot store failed (hydrate or commit).
    #[error("snapshot store failed: {0}")]
    Snapshot(#[source] SS),

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
impl<P, EC, SS, Sub> From<Sub> for ProjectionError<P, EC, SS, Sub> {
    fn from(err: Sub) -> Self {
        Self::Subscription(err)
    }
}
