/// Pluggable serialization for typed values.
///
/// Converts between typed values and byte payloads. The type parameter
/// `E` is the concrete type — implementors add whatever bounds they
/// need (e.g. `Serialize + DeserializeOwned` for JSON codecs).
///
/// Used for both domain events (where `name` = event type from
/// `DomainEvent::name()`) and aggregate snapshots (where `name` =
/// aggregate identifier via `Display`).
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

    /// Serialize a typed value to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the value cannot be serialized.
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Deserialize bytes back to a typed value.
    ///
    /// `name` identifies the type being decoded — for events this is the
    /// variant name (from `DomainEvent::name()`), for snapshots this is
    /// the aggregate identifier. Provided so the codec can discriminate
    /// which variant to construct.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload cannot be deserialized.
    fn decode(&self, name: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
