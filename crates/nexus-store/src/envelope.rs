use crate::error::InvalidSchemaVersion;
use nexus::Version;

// =============================================================================
// PendingEnvelope — typestate builder ensures compile-time valid construction
// =============================================================================

/// Event envelope for the write path — fully owned, going to the database.
///
/// Cannot be constructed directly. Use the typestate builder:
/// ```ignore
/// PendingEnvelopeBuilder::new("stream-1")
///     .version(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .metadata(meta)
///     .build()
/// ```
///
/// Each step returns a different type — the compiler enforces that all
/// required fields are set before `build()` is callable.
#[derive(Debug)]
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    schema_version: u32,
    payload: Vec<u8>,
    metadata: M,
}

impl<M> PendingEnvelope<M> {
    /// The event stream identifier.
    #[must_use]
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The event version in the stream.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The event type name (from `DomainEvent::name()`).
    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        self.event_type
    }

    /// The serialized event payload.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// User-defined metadata.
    #[must_use]
    pub const fn metadata(&self) -> &M {
        &self.metadata
    }

    /// The schema version of the event.
    ///
    /// Defaults to 1 when not explicitly set via the builder.
    /// The `EventStore` save path sets this based on the upcaster chain
    /// to prevent upcasters from double-transforming new events.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

// =============================================================================
// Typestate builder — compile-time enforced construction
// =============================================================================

/// Step 1: has `stream_id`, needs version.
pub struct WithStreamId {
    stream_id: String,
}

/// Step 2: has `stream_id` + `version`, needs `event_type`.
pub struct WithVersion {
    stream_id: String,
    version: Version,
}

/// Step 3: has `stream_id` + `version` + `event_type`, needs payload.
pub struct WithEventType {
    stream_id: String,
    version: Version,
    event_type: &'static str,
}

/// Step 4: has all core fields, needs metadata.
pub struct WithPayload {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    schema_version: u32,
}

impl WithStreamId {
    /// Start building a `PendingEnvelope` with the stream ID.
    #[must_use]
    pub const fn new(stream_id: String) -> Self {
        Self { stream_id }
    }

    /// Set the event version.
    #[must_use]
    pub fn version(self, version: Version) -> WithVersion {
        WithVersion {
            stream_id: self.stream_id,
            version,
        }
    }
}

impl WithVersion {
    /// Set the event type name (from `DomainEvent::name()`).
    #[must_use]
    pub fn event_type(self, event_type: &'static str) -> WithEventType {
        WithEventType {
            stream_id: self.stream_id,
            version: self.version,
            event_type,
        }
    }
}

impl WithEventType {
    /// Set the serialized event payload.
    #[must_use]
    pub fn payload(self, payload: Vec<u8>) -> WithPayload {
        WithPayload {
            stream_id: self.stream_id,
            version: self.version,
            event_type: self.event_type,
            payload,
            schema_version: 1,
        }
    }
}

impl WithPayload {
    /// Override the schema version (default: 1).
    ///
    /// The `EventStore` save path uses this to stamp new events at the
    /// current schema version, preventing upcasters from re-transforming
    /// events that are already in the latest format.
    #[must_use]
    pub const fn schema_version(mut self, schema_version: u32) -> Self {
        self.schema_version = schema_version;
        self
    }

    /// Build with user-defined metadata.
    ///
    /// # Validation contract
    ///
    /// The builder deliberately accepts empty `stream_id` and `event_type`.
    /// Content validation (e.g. rejecting empty strings) is the responsibility
    /// of the `EventStore` facade, not the envelope builder. This keeps the
    /// builder simple and composable while deferring policy to the layer that
    /// owns it. See `security_tests.rs` C2 tests for the documented contract.
    #[must_use]
    pub fn build<M>(self, metadata: M) -> PendingEnvelope<M> {
        PendingEnvelope {
            stream_id: self.stream_id,
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata,
        }
    }

    /// Build with no metadata (`M = ()`).
    ///
    /// See [`Self::build`] for the validation contract — the same rules apply.
    #[must_use]
    pub fn build_without_metadata(self) -> PendingEnvelope<()> {
        self.build(())
    }
}

/// Start building a `PendingEnvelope`.
///
/// ```ignore
/// pending_envelope("stream-1".into())
///     .version(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .build(metadata)
/// ```
#[must_use]
pub const fn pending_envelope(stream_id: String) -> WithStreamId {
    WithStreamId::new(stream_id)
}

/// Event envelope for the read path — borrows from database row buffer.
///
/// Zero allocation for core fields (`stream_id`, `event_type`, `payload`).
/// Metadata `M` is always owned (small data like UUIDs, timestamps).
///
/// The lifetime `'a` ties the envelope to the row buffer — the envelope
/// must be dropped before the cursor advances to the next row.
#[derive(Debug)]
pub struct PersistedEnvelope<'a, M = ()> {
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
}

impl<'a, M> PersistedEnvelope<'a, M> {
    /// Construct a persisted envelope from database row data.
    ///
    /// Adapters call this to wrap raw row data. Once constructed, the
    /// envelope is frozen — all access is through `const` borrowing
    /// accessors. No mutation path exists.
    ///
    /// # Panics
    ///
    /// Panics if `schema_version` is 0 — schema versions start at 1.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "flat constructor mirrors DB row; a builder would add needless complexity for a read-path type"
    )]
    pub const fn new(
        stream_id: &'a str,
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
            stream_id,
            version,
            event_type,
            schema_version,
            payload,
            metadata,
        }
    }

    /// Fallible constructor — returns `Err` if `schema_version` is 0.
    ///
    /// Prefer this over [`new()`](Self::new) in adapter code where you
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
        stream_id: &'a str,
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
            stream_id,
            version,
            event_type,
            schema_version,
            payload,
            metadata,
        })
    }

    /// The event stream identifier.
    #[must_use]
    pub const fn stream_id(&self) -> &str {
        self.stream_id
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

    /// The schema version of the persisted event.
    ///
    /// Used by `EventUpcaster` to decide which events need upgrading.
    /// Always >= 1.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
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
