use std::num::NonZeroU32;

use nexus::Version;

/// Versioned state payload — used for both read and write paths.
///
/// Generic over `S` — the adapter decides serialization format.
/// For byte-level adapters (fjall): `State<Vec<u8>>`.
/// For typed adapters (postgres): `State<MyDomainState>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State<S> {
    version: Version,
    schema_version: NonZeroU32,
    state: S,
}

impl<S> State<S> {
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
