use std::num::NonZeroU32;

use nexus::Version;

/// Persisted state loaded from storage (read path).
///
/// Generic over `S` — byte-level adapters return `PersistedState<Vec<u8>>`,
/// typed adapters return `PersistedState<MyState>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedState<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> PersistedState<S> {
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
