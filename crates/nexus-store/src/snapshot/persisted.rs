use std::num::NonZeroU32;

use nexus::Version;

/// Persisted snapshot loaded from storage (read path).
///
/// Owns the payload bytes. This is the simplified owned variant —
/// a borrowing variant with GAT lifetimes can be added later if
/// benchmarks show the extra allocation matters for specific backends.
#[derive(Debug, Clone)]
pub struct PersistedSnapshot {
    version: Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl PersistedSnapshot {
    /// Create a new persisted snapshot.
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
