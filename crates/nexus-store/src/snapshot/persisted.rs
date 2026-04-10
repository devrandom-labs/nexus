use nexus::Version;

use crate::error::InvalidSchemaVersion;

/// Persisted snapshot loaded from storage (read path).
///
/// Owns the payload bytes. This is the simplified owned variant —
/// a borrowing variant with GAT lifetimes can be added later if
/// benchmarks show the extra allocation matters for specific backends.
#[derive(Debug, Clone)]
pub struct PersistedSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl PersistedSnapshot {
    /// Create a new persisted snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0.
    #[must_use]
    pub fn new(version: Version, schema_version: u32, payload: Vec<u8>) -> Self {
        match Self::try_new(version, schema_version, payload) {
            Ok(snap) => snap,
            Err(e) => panic!("{e}"),
        }
    }

    /// Try to create a new persisted snapshot.
    pub fn try_new(
        version: Version,
        schema_version: u32,
        payload: Vec<u8>,
    ) -> Result<Self, InvalidSchemaVersion> {
        if schema_version == 0 {
            return Err(InvalidSchemaVersion);
        }
        Ok(Self {
            version,
            schema_version,
            payload,
        })
    }

    /// The aggregate version at the time of the snapshot.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The schema version for invalidation checking.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The serialized state bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
