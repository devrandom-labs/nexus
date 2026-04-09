use crate::error::InvalidSchemaVersion;
use nexus::Version;

/// Event envelope for the read path — borrows from database row buffer.
///
/// Zero allocation for core fields (`event_type`, `payload`).
/// Metadata `M` is always owned (small data like UUIDs, timestamps).
///
/// The lifetime `'a` ties the envelope to the row buffer — the envelope
/// must be dropped before the cursor advances to the next row.
#[derive(Debug)]
pub struct PersistedEnvelope<'a, M = ()> {
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
}

impl<'a, M> PersistedEnvelope<'a, M> {
    /// Construct a persisted envelope from database row data (panicking).
    ///
    /// Prefer [`try_new()`](Self::try_new) in adapter code where you
    /// want to surface the error instead of panicking.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0 — schema versions start at 1.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "flat constructor mirrors DB row; a builder would add needless complexity for a read-path type"
    )]
    pub const fn new_unchecked(
        version: Version,
        event_type: &'a str,
        schema_version: u32,
        payload: &'a [u8],
        metadata: M,
    ) -> Self {
        assert!(
            schema_version > 0,
            "schema_version must be > 0: a schema version of 0 is invalid"
        );
        Self {
            version,
            event_type,
            schema_version,
            payload,
            metadata,
        }
    }

    /// Fallible constructor — returns `Err` if `schema_version` is 0.
    ///
    /// Prefer this over [`new_unchecked()`](Self::new_unchecked) in adapter code where you
    /// want to surface the error instead of panicking.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSchemaVersion`] if `schema_version` is 0.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors new() — flat constructor for DB row data"
    )]
    pub fn try_new(
        version: Version,
        event_type: &'a str,
        schema_version: u32,
        payload: &'a [u8],
        metadata: M,
    ) -> Result<Self, InvalidSchemaVersion> {
        if schema_version == 0 {
            return Err(InvalidSchemaVersion);
        }
        Ok(Self {
            version,
            event_type,
            schema_version,
            payload,
            metadata,
        })
    }

    /// The event version in the stream.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The event type name.
    #[must_use]
    pub const fn event_type(&self) -> &str {
        self.event_type
    }

    /// The schema version of the persisted event (raw `u32`).
    ///
    /// Prefer [`schema_version_as_version`](Self::schema_version_as_version) for
    /// passing to upcaster APIs which require `Version`.
    /// Always >= 1.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The schema version as a [`Version`] for upcaster APIs.
    ///
    /// Safe because constructors enforce `schema_version >= 1`.
    ///
    /// # Panics
    ///
    /// Panics if the internal `schema_version` is zero, which violates the
    /// constructor invariant and should never happen in practice.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "constructors guarantee schema_version >= 1"
    )]
    pub fn schema_version_as_version(&self) -> Version {
        Version::new(u64::from(self.schema_version))
            .expect("PersistedEnvelope invariant: schema_version >= 1")
    }

    /// The serialized event payload.
    #[must_use]
    pub const fn payload(&self) -> &[u8] {
        self.payload
    }

    /// User-defined metadata.
    #[must_use]
    pub const fn metadata(&self) -> &M {
        &self.metadata
    }
}
