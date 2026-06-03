// ═══════════════════════════════════════════════════════════════════════════
// Encode<E> — serialize a typed value to bytes
// ═══════════════════════════════════════════════════════════════════════════

use crate::envelope::PersistedEnvelope;

/// Serialize a typed value to bytes.
///
/// The write half of the codec story. Independent from [`Decode`] so
/// write-only adapters need not implement decoding, and so the encode path
/// can be bounded without forcing a decode strategy on the caller.
///
/// `E: ?Sized` allows unsized event types (e.g. `Archived<MyEvent>`).
pub trait Encode<E: ?Sized>: Send + Sync + 'static {
    /// The error type for serialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a typed value to bytes.
    ///
    /// Returns [`bytes::Bytes`] so the encoded payload can flow through
    /// the store and wire layer without an intermediate `Vec<u8> → Bytes`
    /// copy. Codecs that produce a `Vec<u8>` internally adapt via
    /// `Bytes::from(vec)` (zero-copy ownership transfer of the backing
    /// allocation); codecs that build incrementally may use
    /// `BytesMut::freeze()`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the value cannot be serialized.
    fn encode(&self, event: &E) -> Result<bytes::Bytes, Self::Error>;
}

// ═══════════════════════════════════════════════════════════════════════════
// Decode<E> — deserialize a persisted envelope to a typed value (or borrow into it)
// ═══════════════════════════════════════════════════════════════════════════

/// Decode a persisted envelope to a typed value (or borrow into it).
///
/// One trait covers both owning and borrowing codecs via the
/// [`Output<'a>`](Self::Output) GAT:
///
/// - **Owning codecs** (serde JSON/bincode/postcard): `type Output<'a> = E`.
///   `decode` returns an owned `E` that lives past the envelope.
/// - **Borrowing codecs** (rkyv): `type Output<'a> = &'a <E as Archive>::Archived`.
///   `decode` returns a reference into the envelope's payload bytes — no
///   allocation, no copy.
/// - **Plain-old-data codecs** (bytemuck): `type Output<'a> = &'a E`.
///   Reinterprets the payload as `&E` directly. Relies on the wire-format
///   16-byte payload-alignment invariant for safety.
///
/// The two-trait split (previously `Decode` + `BorrowingDecode`) was a
/// workaround for the borrowed-cursor lifetime cliff: when the envelope
/// was borrowed from a cursor row that died on `.next()`, the same
/// operation needed two trait shapes. The owned-`Bytes` envelope removes
/// the cliff — `'a` now ties to the envelope itself, which is cheap-to-
/// clone and lifetime-independent.
///
/// `E: ?Sized` allows unsized event types: `Decode<[u8]>`, `Decode<str>`,
/// `Decode<<E as Archive>::Archived>`.
///
/// Independent from [`Encode`]: a codec may implement only `Decode`
/// (read-only replica), only `Encode` (write-only shipper), or both.
pub trait Decode<E: ?Sized>: Send + Sync + 'static {
    /// What [`decode`](Self::decode) returns.
    ///
    /// `E` for owning codecs, `&'a E` (or `&'a Archived<E>`) for
    /// zero-copy codecs. The `where Self: 'a` bound is the standard GAT
    /// shape and adds no real constraint for `'static` codecs.
    type Output<'a>
    where
        Self: 'a;

    /// The error type for deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Decode the envelope's payload to [`Output<'a>`](Self::Output).
    ///
    /// The envelope is the input: its [`event_type()`](PersistedEnvelope::event_type)
    /// is the variant discriminant, [`payload()`](PersistedEnvelope::payload)
    /// is the serialized bytes. Codecs reach into the envelope for whatever
    /// they need — no pre-extracted arguments.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload is invalid (e.g. failed archive
    /// validation for rkyv) or does not match the type discriminant.
    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<Self::Output<'a>, Self::Error>;
}

// ═══════════════════════════════════════════════════════════════════════════
// Serde adapter — feature-gated Encode/Decode impls driven by a SerdeFormat
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "serde")]
pub mod serde {
    use ::serde::{Serialize, de::DeserializeOwned};

    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;

    /// Format-agnostic serialization strategy for serde-compatible events.
    ///
    /// Implementors provide the wire format (JSON, bincode, postcard, etc.)
    /// while [`SerdeCodec`] handles the plumbing to satisfy [`Encode`] and
    /// [`Decode`].
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
    /// store's [`Encode`] and [`Decode`] traits. Both directions share
    /// the same underlying format and thus the same error type.
    ///
    /// # Variant dispatch
    ///
    /// `SerdeCodec` ignores [`env.event_type()`](PersistedEnvelope::event_type)
    /// on `decode`. Serde formats embed variant discriminants in the payload
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

    impl<E, F> Encode<E> for SerdeCodec<F>
    where
        E: Serialize + Send + Sync + 'static,
        F: SerdeFormat,
    {
        type Error = F::Error;

        fn encode(&self, event: &E) -> Result<bytes::Bytes, Self::Error> {
            self.format.serialize(event).map(bytes::Bytes::from)
        }
    }

    impl<E, F> Decode<E> for SerdeCodec<F>
    where
        E: DeserializeOwned + Send + Sync + 'static,
        F: SerdeFormat,
    {
        type Output<'a>
            = E
        where
            Self: 'a;
        type Error = F::Error;

        fn decode<'a>(
            &'a self,
            env: &'a PersistedEnvelope,
        ) -> Result<Self::Output<'a>, Self::Error> {
            self.format.deserialize(env.payload())
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
