use nexus::Version;

/// Event envelope for the write path — fully owned, going to the database.
///
/// Constructed internally by the `EventStore` facade from typed events
/// and codec output. All fields are private with read-only accessors.
///
/// `M` is user-defined metadata (correlation ID, tenant ID, timestamps, etc.).
/// Default is `()` for bare minimum storage.
#[derive(Debug)]
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    metadata: M,
}

impl<M> PendingEnvelope<M> {
    /// Construct a new pending envelope.
    ///
    /// # Panics
    ///
    /// Panics if `stream_id` is empty or `event_type` is empty.
    #[must_use]
    #[allow(
        clippy::panic,
        reason = "invalid envelope construction must crash — silent corruption is worse"
    )]
    pub fn new(
        stream_id: String,
        version: Version,
        event_type: &'static str,
        payload: Vec<u8>,
        metadata: M,
    ) -> Self {
        assert!(!stream_id.is_empty(), "stream_id must not be empty");
        assert!(!event_type.is_empty(), "event_type must not be empty");
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
    /// Construct a new persisted envelope.
    #[must_use]
    pub const fn new(
        stream_id: &'a str,
        version: Version,
        event_type: &'a str,
        payload: &'a [u8],
        metadata: M,
    ) -> Self {
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
