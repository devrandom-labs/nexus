use std::num::NonZeroU32;

use nexus::Version;

/// Projection state payload ready for persistence (write path).
///
/// Contains the serialized projection state, the stream version at
/// state computation time, and a schema version for invalidation.
#[derive(Debug, Clone)]
pub struct PendingState {
    version: Version,
    schema_version: NonZeroU32,
    payload: Vec<u8>,
}

impl PendingState {
    /// Create a new pending projection state.
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, payload: Vec<u8>) -> Self {
        Self {
            version,
            schema_version,
            payload,
        }
    }

    /// The stream version at the time of state computation.
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
