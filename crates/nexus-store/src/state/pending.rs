use std::num::NonZeroU32;

use nexus::Version;

/// State payload ready for persistence (write path).
///
/// Generic over `S` — the adapter decides serialization format.
/// For byte-level adapters (fjall): `PendingState<Vec<u8>>`.
/// For typed adapters (postgres): `PendingState<MyState>`.
#[derive(Debug, Clone)]
pub struct PendingState<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> PendingState<S> {
    #[must_use]
    pub const fn new(version: Version, schema_version: NonZeroU32, state: S) -> Self {
        Self {
            version,
            schema_version,
            state,
        }
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn schema_version(&self) -> NonZeroU32 {
        self.schema_version
    }

    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    #[must_use]
    pub fn into_parts(self) -> (Version, NonZeroU32, S) {
        (self.version, self.schema_version, self.state)
    }
}
