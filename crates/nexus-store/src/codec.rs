use nexus::DomainEvent;

/// Pluggable serialization for domain events.
///
/// Converts between typed events and byte payloads. Monomorphized via
/// type parameter on `EventStore` for zero-cost at the call site.
///
/// Knows nothing about envelopes, streams, versions, or metadata.
/// Just bytes in, bytes out.
///
/// Users implement this for their chosen format (`serde_json`, postcard,
/// musli, protobuf, rkyv, etc.).
pub trait Codec: Send + Sync + 'static {
    /// The error type for serialization/deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a typed domain event to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the event cannot be serialized.
    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Deserialize bytes back to a typed domain event.
    ///
    /// `event_type` is the variant name (from `DomainEvent::name()`),
    /// provided so the codec knows which variant to construct.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload cannot be deserialized into
    /// the target event type.
    fn decode<E: DomainEvent>(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
