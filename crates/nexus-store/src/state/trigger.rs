use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to persist state.
///
/// Used by both projection runners (when to checkpoint projection state)
/// and snapshot decorators (when to snapshot aggregate state).
pub trait PersistTrigger: Send + Sync {
    /// Whether state should be persisted now.
    ///
    /// - `old_version`: version before the operation (`None` for first run)
    /// - `new_version`: version after the operation
    /// - `event_names`: names of events just processed
    fn should_persist(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Persist every N events (bucket-crossing algorithm).
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl PersistTrigger for EveryNEvents {
    fn should_persist(
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

/// Persist after specific event types.
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

impl PersistTrigger for AfterEventTypes {
    fn should_persist(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        mut event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}
