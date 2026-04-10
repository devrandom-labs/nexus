use nexus::Version;

use crate::error::InvalidSchemaVersion;

/// Snapshot payload ready for persistence (write path).
///
/// Contains the serialized aggregate state, the aggregate version at
/// snapshot time, and a schema version for invalidation.
#[derive(Debug, Clone)]
pub struct PendingSnapshot {
    version: Version,
    schema_version: u32,
    payload: Vec<u8>,
}

impl PendingSnapshot {
    /// Create a new pending snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0. Use [`try_new`](Self::try_new)
    /// for a fallible alternative.
    #[must_use]
    #[allow(
        clippy::panic,
        reason = "convenience constructor with documented panic"
    )]
    pub fn new(version: Version, schema_version: u32, payload: Vec<u8>) -> Self {
        match Self::try_new(version, schema_version, payload) {
            Ok(snap) => snap,
            Err(e) => panic!("{e}"),
        }
    }

    /// Try to create a new pending snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSchemaVersion`] if `schema_version` is 0.
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
