/// Zero-copy codec for domain events.
///
/// Unlike [`Codec<E>`](crate::Codec) which returns an owned `E`,
/// `BorrowingCodec` returns `&'a E` borrowing directly from the payload
/// buffer. This enables zero-allocation event streaming for codecs
/// like rkyv and flatbuffers where the serialized bytes ARE the data.
///
/// `E: ?Sized` allows unsized event types (e.g. `Archived<MyEvent>`).
///
/// # When to use
///
/// - **Use `Codec<E>`** for serde-based formats (JSON, bincode, postcard)
///   where deserialization produces an owned value.
/// - **Use `BorrowingCodec<E>`** for zero-copy formats (rkyv, flatbuffers)
///   where the serialized bytes can be reinterpreted in-place.
pub trait BorrowingCodec<E: ?Sized>: Send + Sync + 'static {
    /// The error type for serialization/deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a domain event to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the event cannot be serialized.
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Decode bytes by borrowing directly from the payload buffer.
    ///
    /// The returned reference has lifetime `'a` tied to `payload` —
    /// it borrows from the cursor's row buffer. No allocation occurs.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload is invalid (e.g. failed
    /// archive validation for rkyv).
    fn decode<'a>(&self, event_type: &str, payload: &'a [u8]) -> Result<&'a E, Self::Error>;
}
