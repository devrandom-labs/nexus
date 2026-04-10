use std::num::NonZeroU32;

use nexus::Version;

/// Snapshot payload ready for persistence (write path).
///
/// Contains the serialized aggregate state, the aggregate version at
/// snapshot time, and a schema version for invalidation.
#[derive(Debug, Clone)]
pub struct PendingSnapshot {
    version: Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl PendingSnapshot {
    /// Create a new pending snapshot.
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, payload: Vec<u8>) -> Self {
        Self {
            version,
            schema_version,
            payload,
        }
    }

    /// The aggregate version at the time of the snapshot.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
