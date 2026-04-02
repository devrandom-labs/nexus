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
    #[must_use]
    pub const fn new(
        stream_id: String,
        version: Version,
        event_type: &'static str,
        payload: Vec<u8>,
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
