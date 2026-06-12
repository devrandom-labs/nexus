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

/// Routes the subscription cursor's `Error` into
/// [`ProjectionError::Subscription`]. The adapter trait
/// [`nexus_store::subscription::RawSubscription`] guarantees
/// `<Self::Stream as EventStream>::Error == Self::Error`, so a single
/// `From` impl threads stream errors through the run loop without extra
/// glue at the call site.
impl<P, EC, SS, Sub> From<Sub> for ProjectionError<P, EC, SS, Sub> {
    fn from(err: Sub) -> Self {
        Self::Subscription(err)
    }
}
