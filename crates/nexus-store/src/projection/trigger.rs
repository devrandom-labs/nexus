use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to update projection state.
///
/// Checked after each successful `save()`. The trigger receives both the
/// old and new version (to detect boundary crossings regardless of batch
/// size) and the names of events just persisted (for semantic triggers).
pub trait ProjectionTrigger: Send + Sync {
    /// Whether a projection state update should happen after this save.
    ///
    /// - `old_version`: version before save (`None` for new streams)
    /// - `new_version`: version after save
    /// - `event_names`: names of events just persisted (from `DomainEvent::name()`).
    fn should_project(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Trigger a projection state update every N events.
///
/// Uses a bucket-crossing algorithm to detect when a save crosses an
/// N-event boundary, regardless of batch size.
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl ProjectionTrigger for EveryNEvents {
    fn should_project(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        _event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        let n = self.0.get();
        let old_bucket = old_version.map_or(0, |v| v.as_u64() / n);
        let new_bucket = new_version.as_u64() / n;
        new_bucket > old_bucket
    }
}

/// Trigger a projection state update after specific event types.
///
/// Useful for domain milestones where a projection update aligns with
/// business semantics.
#[derive(Debug, Clone)]
pub struct AfterEventTypes {
    types: Vec<&'static str>,
}

impl AfterEventTypes {
    /// Create a trigger that fires when any of the given event types is persisted.
    #[must_use]
    pub fn new(types: &[&'static str]) -> Self {
        Self {
            types: types.to_vec(),
        }
    }
}

impl ProjectionTrigger for AfterEventTypes {
    fn should_project(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        mut event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}
