// ═══════════════════════════════════════════════════════════════════════════
// Codec<E> — owning serialization (serde-based formats)
// ═══════════════════════════════════════════════════════════════════════════

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
/// in-place, use [`BorrowingCodec<E>`] instead.
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

// ═══════════════════════════════════════════════════════════════════════════
// BorrowingCodec<E> — zero-copy decode (rkyv, flatbuffers)
// ═══════════════════════════════════════════════════════════════════════════

/// Zero-copy codec for typed values.
///
/// Unlike [`Codec<E>`] which returns an owned `E`,
/// `BorrowingCodec` returns `&'a E` borrowing directly from the payload
/// buffer. This enables zero-allocation event streaming for codecs
/// like rkyv and flatbuffers where the serialized bytes ARE the data.
///
/// `E: ?Sized` allows unsized event types (e.g. `Archived<MyEvent>`).
///
/// Used for both domain events (where `name` = event type) and
/// aggregate snapshots (where `name` = aggregate identifier).
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

    /// Serialize a typed value to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the value cannot be serialized.
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Decode bytes by borrowing directly from the payload buffer.
    ///
    /// `name` identifies the type being decoded — for events this is the
    /// variant name, for snapshots this is the aggregate identifier.
    ///
    /// The returned reference has lifetime `'a` tied to `payload` —
    /// it borrows from the cursor's row buffer. No allocation occurs.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload is invalid (e.g. failed
    /// archive validation for rkyv).
    fn decode<'a>(&self, name: &str, payload: &'a [u8]) -> Result<&'a E, Self::Error>;
}

// ═══════════════════════════════════════════════════════════════════════════
// Serde adapter — feature-gated `Codec<E>` impl driven by a `SerdeFormat`
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "serde")]
pub mod serde {
    use ::serde::{Serialize, de::DeserializeOwned};

    use super::Codec;

    /// Format-agnostic serialization strategy for serde-compatible events.
    ///
    /// Implementors provide the wire format (JSON, bincode, postcard, etc.)
    /// while [`SerdeCodec`] handles the plumbing to satisfy [`Codec<E>`].
    ///
    /// # Implementor contract
    ///
    /// - `serialize` and `deserialize` must be inverses: for any `T`,
    ///   `deserialize(serialize(t)?) == t`.
    /// - Errors must accurately describe the failure (not erase the cause).
    pub trait SerdeFormat: Send + Sync + 'static {
        /// The error type for serialization/deserialization failures.
        type Error: std::error::Error + Send + Sync + 'static;

        /// Serialize a value to bytes.
        ///
        /// # Errors
        ///
        /// Returns `Self::Error` if the value cannot be serialized.
        fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, Self::Error>;

        /// Deserialize bytes back to a typed value.
        ///
        /// # Errors
        ///
        /// Returns `Self::Error` if the payload cannot be deserialized.
        fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, Self::Error>;
    }

    /// Generic serde-based codec parameterized by a [`SerdeFormat`].
    ///
    /// Wraps any `SerdeFormat` implementation and bridges it to the
    /// store's [`Codec<E>`] trait. The format handles the wire encoding
    /// while `SerdeCodec` satisfies the event store contract.
    ///
    /// # Variant dispatch
    ///
    /// Unlike raw `Codec<E>` implementations that may use `name` to
    /// select which variant to deserialize, `SerdeCodec` ignores `name`
    /// entirely. Serde formats embed variant discriminants in the payload
    /// (e.g. `{"Credited": {...}}` in JSON), so the codec delegates dispatch
    /// to serde itself.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use nexus_store::{JsonCodec, Json, SerdeCodec};
    ///
    /// // Via the JsonCodec alias:
    /// let codec = JsonCodec::default();
    ///
    /// // Or construct manually with any SerdeFormat:
    /// let codec = SerdeCodec::new(Json);
    /// ```
    pub struct SerdeCodec<F> {
        format: F,
    }

    impl<F> SerdeCodec<F> {
        /// Create a new `SerdeCodec` wrapping the given format.
        pub const fn new(format: F) -> Self {
            Self { format }
        }
    }

    impl<F: Default> Default for SerdeCodec<F> {
        fn default() -> Self {
            Self::new(F::default())
        }
    }

    impl<E, F> Codec<E> for SerdeCodec<F>
    where
        E: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: SerdeFormat,
    {
        type Error = F::Error;

        fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error> {
            self.format.serialize(event)
        }

        fn decode(&self, _name: &str, payload: &[u8]) -> Result<E, Self::Error> {
            self.format.deserialize(payload)
        }
    }

    /// Seal `Debug` — show the format type, not its internals.
    impl<F> std::fmt::Debug for SerdeCodec<F> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SerdeCodec")
                .field("format", &std::any::type_name::<F>())
                .finish()
        }
    }

    #[cfg(feature = "json")]
    pub mod json {
        use ::serde::{Serialize, de::DeserializeOwned};

        use super::{SerdeCodec, SerdeFormat};

        /// JSON wire format backed by `serde_json`.
        ///
        /// Use directly with [`SerdeCodec`] or via the [`JsonCodec`] alias.
        #[derive(Debug, Clone, Copy, Default)]
        pub struct Json;

        impl SerdeFormat for Json {
            type Error = serde_json::Error;

            fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
                serde_json::to_vec(value)
            }

            fn deserialize<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, Self::Error> {
                serde_json::from_slice(bytes)
            }
        }

        /// Convenience alias: a [`SerdeCodec`] using [`Json`] format.
        pub type JsonCodec = SerdeCodec<Json>;
    }
}
