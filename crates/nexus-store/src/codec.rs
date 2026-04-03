/// Pluggable serialization for domain events.
///
/// Converts between typed events and byte payloads. The type parameter
/// `E` is the concrete event enum — implementors add whatever bounds
/// they need (e.g. `Serialize + DeserializeOwned` for JSON codecs).
///
/// Knows nothing about envelopes, streams, versions, or metadata.
/// Just bytes in, bytes out.
///
/// # When to use
///
/// Use `Codec<E>` for serde-based formats (JSON, bincode, postcard)
/// where deserialization produces an owned value. For zero-copy formats
/// (rkyv, flatbuffers) where the serialized bytes can be reinterpreted
/// in-place, use [`BorrowingCodec<E>`](crate::BorrowingCodec) instead.
pub trait Codec<E>: Send + Sync + 'static {
    /// The error type for serialization/deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a typed domain event to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the event cannot be serialized.
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Deserialize bytes back to a typed domain event.
    ///
    /// `event_type` is the variant name (from `DomainEvent::name()`),
    /// provided so the codec knows which variant to construct.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload cannot be deserialized into
    /// the target event type.
    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
