use super::morsel::EventMorsel;
use crate::error::UpcastError;
use nexus::Version;

/// Upcast old events to the current schema version.
///
/// `EventStore` calls `apply()` on the read path and `current_version()`
/// on the write path. The proc macro `#[nexus::transforms]` generates
/// implementations; `()` is the no-op passthrough.
pub trait Upcaster: Send + Sync {
    /// The error type produced by transform functions.
    ///
    /// Each upcaster implementation specifies its own concrete error type.
    /// The no-op `()` impl uses [`Infallible`](std::convert::Infallible).
    type Error: std::error::Error + Send + Sync + 'static;

    /// Run all matching transforms until the morsel is at the current schema version.
    ///
    /// # Errors
    ///
    /// Returns [`UpcastError`] if a transform function fails or validation detects
    /// an invariant violation (version not advanced, empty event type, chain limit).
    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>>;

    /// Current schema version for an event type (stamped on new events).
    /// Returns `None` if the event type has no transforms.
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

/// No-op upcaster — passthrough, no transforms.
impl Upcaster for () {
    type Error = std::convert::Infallible;

    #[inline]
    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        Ok(morsel)
    }

    #[inline]
    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}
