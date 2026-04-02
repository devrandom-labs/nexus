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
        }
    }
}

impl WithPayload {
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
    payload: &'a [u8],
    metadata: M,
}

impl<'a, M> PersistedEnvelope<'a, M> {
    /// Construct a new persisted envelope from database row data.
    ///
    /// # Debug assertions
    ///
    /// In debug/test builds, asserts that `stream_id` and `event_type` are
    /// non-empty. These are invariants of the read path — database adapters
    /// must never store or return empty identifiers. The assertions are
    /// zero-cost in release builds.
    #[must_use]
    pub fn new(
        stream_id: &'a str,
        version: Version,
        event_type: &'a str,
        payload: &'a [u8],
        metadata: M,
    ) -> Self {
        debug_assert!(
            !stream_id.is_empty(),
            "PersistedEnvelope should not be constructed with empty stream_id \
             — database adapters must validate"
        );
        debug_assert!(
            !event_type.is_empty(),
            "PersistedEnvelope should not be constructed with empty event_type \
             — database adapters must validate"
        );
        Self {
            stream_id,
            version,
            event_type,
            payload,
            metadata,
        }
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
