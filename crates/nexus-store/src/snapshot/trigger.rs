use std::num::NonZeroU64;

use nexus::Version;

/// Strategy for deciding when to take a snapshot.
///
/// Checked after each successful `save()`. The trigger receives both the
/// old and new aggregate version (to detect boundary crossings regardless
/// of batch size) and the names of events just persisted (for semantic
/// triggers).
pub trait SnapshotTrigger: Send + Sync {
    /// Whether a snapshot should be taken after this save.
    ///
    /// - `old_version`: aggregate version before save (`None` for new aggregates)
    /// - `new_version`: aggregate version after save
    /// - `event_names`: names of events just persisted (from `DomainEvent::name()`).
    ///   Passed as a mutable iterator to avoid allocation at the call site.
    ///   Implementations that don't need event names should ignore this parameter.
    fn should_snapshot(
        &self,
        old_version: Option<Version>,
        new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool;
}

/// Trigger a snapshot every N events.
///
/// Detects when a save crosses an N-event boundary, regardless of batch
/// size. For example, `EveryNEvents(100)` triggers when the aggregate
/// crosses version 100, 200, 300, etc. — even if the batch was 7 events
/// jumping from version 96 to 103.
///
/// Uses a bucket-crossing algorithm instead of modulo to avoid missing
/// boundaries when batches skip over exact multiples.
#[derive(Debug, Clone, Copy)]
pub struct EveryNEvents(pub NonZeroU64);

impl SnapshotTrigger for EveryNEvents {
    fn should_snapshot(
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

/// Trigger a snapshot after specific event types are persisted.
///
/// Useful for domain milestones (e.g., `OrderCompleted`, `AccountClosed`)
/// where the aggregate is unlikely to change further, or where a stable
/// snapshot point aligns with business semantics.
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

impl SnapshotTrigger for AfterEventTypes {
    fn should_snapshot(
        &self,
        _old_version: Option<Version>,
        _new_version: Version,
        event_names: impl Iterator<Item: AsRef<str>>,
    ) -> bool {
        event_names.any(|name| self.types.iter().any(|t| *t == name.as_ref()))
    }
}
