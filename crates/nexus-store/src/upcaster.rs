use crate::error::UpcastError;
use crate::morsel::EventMorsel;
use nexus::Version;

/// Upcast old events to the current schema version.
///
/// `EventStore` calls `apply()` on the read path and `current_version()`
/// on the write path. The proc macro `#[nexus::transforms]` generates
/// implementations; `()` is the no-op passthrough.
pub trait Upcaster: Send + Sync {
    /// Run all matching transforms until the morsel is at the current schema version.
    ///
    /// # Errors
    ///
    /// Returns [`UpcastError::TransformFailed`] if a transform function fails.
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError>;

    /// Current schema version for an event type (stamped on new events).
    /// Returns `None` if the event type has no transforms.
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

/// No-op upcaster — passthrough, no transforms.
impl Upcaster for () {
    #[inline]
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        Ok(morsel)
    }

    #[inline]
    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}
