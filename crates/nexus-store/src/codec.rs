//! Serialization traits — [`Encode<E>`] and [`Decode<E>`].
//!
//! Two traits, not three. The previous shape carried a third trait
//! (`BorrowingDecode`) as a workaround for a lifetime cliff in the old
//! borrowed-cursor read path: when the envelope was borrowed from a
//! cursor row that died on the next `.next()`, the same logical
//! operation needed two different trait shapes (one returning `E`, one
//! returning `&'a E`).
//!
//! The owned-[`bytes::Bytes`] envelope from the 2026-05-27 refactor
//! removed that cliff. `PersistedEnvelope` is now cheap-to-clone
//! (`Bytes` is Arc-counted; range copies are 8 bytes each) and has no
//! lifetime parameter, so `'a` ties cleanly to the envelope itself.
//! Both shapes — owning and borrowing — collapse into a single trait
//! with a generic associated type:
//!
//! ```ignore
//! pub trait Decode<E: ?Sized>: Send + Sync + 'static {
//!     type Output<'a> where Self: 'a;
//!     type Error: std::error::Error + Send + Sync + 'static;
//!     fn decode<'a>(&'a self, env: &'a PersistedEnvelope)
//!         -> Result<Self::Output<'a>, Self::Error>;
//! }
//! ```
//!
//! - Owning codec (`SerdeCodec<Json>`): `type Output<'a> = E`.
//! - Archived zero-copy (`rkyv::RkyvCodec`): `type Output<'a> = &'a E::Archived`.
//! - POD zero-copy (`bytemuck::BytemuckCodec`): `type Output<'a> = &'a E`.
//!
//! [`Encode<E>`] stays separate from [`Decode<E>`] so write-only
//! adapters (shippers) and read-only adapters (replicas) need not
//! implement the other half. [`Encode::encode`] returns
//! [`bytes::Bytes`] (not `Vec<u8>`) so the encoded payload flows
//! end-to-end through the store and wire layer without an intermediate
//! `Vec<u8> → Bytes` copy.
//!
//! Both `Encode` and `Decode` take their event type with `E: ?Sized` so
//! the unsized archived types (`Archived<MyEvent>`, `[u8]`, `str`) are
//! representable.

use crate::envelope::PersistedEnvelope;

// ═══════════════════════════════════════════════════════════════════════════
// Encode<E> — serialize a typed value to bytes
// ═══════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════
// Bytemuck — plain-old-data zero-copy codec
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "bytemuck")]
pub mod bytemuck {
    use ::bytemuck::{AnyBitPattern, NoUninit, PodCastError};
    use bytes::Bytes;
    use thiserror::Error;

    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;

    /// Wrapper around [`PodCastError`] that satisfies the `std::error::Error` bound.
    ///
    /// Upstream `PodCastError` does not implement `std::error::Error`
    /// (`bytemuck` is `no_std` by default and skips the impl), so
    /// `Decode::Error` requires a wrapper.
    ///
    /// Captures the upstream error's `Debug` representation as a
    /// stack-allocated [`ArrayString`](arrayvec::ArrayString) — no heap
    /// allocation on the error path.
    #[derive(Debug, Error)]
    #[error("bytemuck cast error: {0}")]
    pub struct BytemuckError(arrayvec::ArrayString<64>);

    impl From<PodCastError> for BytemuckError {
        fn from(value: PodCastError) -> Self {
            use core::fmt::Write;
            let mut buf = arrayvec::ArrayString::<64>::new();
            let _ = write!(buf, "{value:?}");
            Self(buf)
        }
    }

    /// Codec for plain-old-data types.
    ///
    /// Zero-copy on the read path: [`Decode::Output<'a>`] is `&'a E`,
    /// pointing directly into the envelope's payload bytes (which are
    /// 16-byte aligned by the wire-format invariant — see
    /// [`crate::wire`]).
    ///
    /// `E` must implement [`AnyBitPattern`] (every bit pattern is a valid
    /// value of `E`) and [`NoUninit`] (no padding/uninitialized bytes).
    /// In practice this means `#[repr(C)]` POD types with no padding.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[repr(C)]
    /// #[derive(Clone, Copy, bytemuck::AnyBitPattern, bytemuck::NoUninit)]
    /// struct Pos { x: f32, y: f32, z: f32, _pad: f32 }
    ///
    /// let codec = BytemuckCodec;
    /// let bytes = codec.encode(&Pos { x: 1.0, y: 2.0, z: 3.0, _pad: 0.0 })?;
    /// // ... persist to store, read back ...
    /// let pos: &Pos = codec.decode(&env)?;  // borrowed, no allocation
    /// ```
    #[derive(Debug, Default, Clone, Copy)]
    pub struct BytemuckCodec;

    impl<E> Encode<E> for BytemuckCodec
    where
        E: NoUninit + Send + Sync + 'static,
    {
        type Error = core::convert::Infallible;

        fn encode(&self, event: &E) -> Result<Bytes, Self::Error> {
            Ok(Bytes::copy_from_slice(::bytemuck::bytes_of(event)))
        }
    }

    impl<E> Decode<E> for BytemuckCodec
    where
        E: AnyBitPattern + NoUninit + Send + Sync + 'static,
    {
        type Output<'a>
            = &'a E
        where
            Self: 'a;
        type Error = BytemuckError;

        fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<&'a E, Self::Error> {
            ::bytemuck::try_from_bytes(env.payload()).map_err(BytemuckError::from)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::wire;

        #[repr(C)]
        #[derive(
            Clone, Copy, Debug, PartialEq, ::bytemuck::AnyBitPattern, ::bytemuck::NoUninit,
        )]
        struct Pos {
            x: f32,
            y: f32,
            z: f32,
            _pad: f32,
        }

        fn build_test_envelope(payload: &[u8]) -> PersistedEnvelope {
            let frame = wire::encode_frame(
                crate::store::GlobalSeq::INITIAL.as_u64(),
                1,
                "Pos",
                None,
                payload,
            )
            .expect("wire encode_frame ok");
            PersistedEnvelope::try_new(
                nexus::Version::INITIAL,
                crate::store::GlobalSeq::INITIAL,
                frame.value,
                1,
                frame.offsets.event_type,
                frame.offsets.payload,
                None,
            )
            .expect("envelope construction ok")
        }

        #[test]
        fn round_trip_yields_equal_value() {
            let codec = BytemuckCodec;
            let original = Pos {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                _pad: 0.0,
            };

            let bytes = codec.encode(&original).unwrap();
            let env = build_test_envelope(&bytes);
            let decoded: &Pos = codec.decode(&env).unwrap();

            assert_eq!(decoded, &original);
        }

        #[test]
        fn decode_borrows_from_envelope_payload() {
            // Zero-copy assertion: the decoded &Pos points to the same
            // bytes the envelope owns. The pointer equality proves there
            // was no copy.
            let codec = BytemuckCodec;
            let original = Pos {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                _pad: 0.0,
            };

            let bytes = codec.encode(&original).unwrap();
            let env = build_test_envelope(&bytes);
            let decoded: &Pos = codec.decode(&env).unwrap();

            let env_payload_ptr = env.payload().as_ptr();
            let decoded_ptr: *const u8 = std::ptr::from_ref::<Pos>(decoded).cast();
            assert_eq!(
                env_payload_ptr, decoded_ptr,
                "BytemuckCodec must borrow from envelope payload, not copy"
            );
        }

        #[test]
        fn decode_rejects_wrong_size() {
            let codec = BytemuckCodec;
            // 8 bytes instead of 16 (size_of::<Pos>())
            let env = build_test_envelope(&[0u8; 8]);
            let result: Result<&Pos, _> = codec.decode(&env);
            assert!(result.is_err(), "wrong-size payload must be rejected");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Rkyv — archived-data zero-copy codec
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "rkyv")]
pub mod rkyv {
    use ::rkyv::{
        Archive, Serialize,
        api::high::{HighSerializer, HighValidator, to_bytes_in},
        bytecheck::CheckBytes,
        rancor,
        ser::allocator::ArenaHandle,
        util::AlignedVec,
    };
    use bytes::Bytes;

    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;

    /// Zero-copy codec backed by rkyv 0.8.
    ///
    /// - **Encode**: serializes `E` to its archived bytes via
    ///   [`rkyv::to_bytes`]. Adapts `AlignedVec` → `Bytes` via
    ///   `Bytes::from(vec.into_vec())`. (`AlignedVec`'s alignment doesn't
    ///   survive the conversion, but the wire-format aligns the payload
    ///   when the row is built, so the envelope's payload regains
    ///   alignment.)
    /// - **Decode**: validates + accesses the envelope payload as
    ///   `&Archived<E>` via [`rkyv::access`]. Zero-copy on the read
    ///   path; the returned reference borrows directly from
    ///   `env.payload()`.
    ///
    /// `Output<'a> = &'a <E as Archive>::Archived`.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct RkyvCodec;

    impl<E> Encode<E> for RkyvCodec
    where
        E: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>
            + Send
            + Sync
            + 'static,
    {
        type Error = rancor::Error;

        fn encode(&self, event: &E) -> Result<Bytes, Self::Error> {
            let aligned = to_bytes_in::<_, rancor::Error>(event, AlignedVec::new())?;
            // AlignedVec::into_vec → Vec<u8> → Bytes. Alignment is lost
            // here, but wire::encode_frame re-aligns the payload on its way
            // into the envelope.
            Ok(Bytes::from(aligned.into_vec()))
        }
    }

    impl<E> Decode<E> for RkyvCodec
    where
        E: Archive + Send + Sync + 'static,
        E::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>,
    {
        type Output<'a>
            = &'a E::Archived
        where
            Self: 'a;
        type Error = rancor::Error;

        fn decode<'a>(
            &'a self,
            env: &'a PersistedEnvelope,
        ) -> Result<&'a E::Archived, Self::Error> {
            ::rkyv::access::<E::Archived, rancor::Error>(env.payload())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::wire;

        #[derive(::rkyv::Archive, ::rkyv::Serialize, ::rkyv::Deserialize, Debug, PartialEq, Eq)]
        struct Move {
            steps: u32,
            dir: u8,
        }

        fn build_test_envelope(payload: &[u8]) -> PersistedEnvelope {
            let frame = wire::encode_frame(
                crate::store::GlobalSeq::INITIAL.as_u64(),
                1,
                "Move",
                None,
                payload,
            )
            .expect("wire encode_frame ok");
            PersistedEnvelope::try_new(
                nexus::Version::INITIAL,
                crate::store::GlobalSeq::INITIAL,
                frame.value,
                1,
                frame.offsets.event_type,
                frame.offsets.payload,
                None,
            )
            .expect("envelope construction ok")
        }

        #[test]
        fn round_trip_yields_equal_archived_fields() {
            let codec = RkyvCodec;
            let original = Move { steps: 42, dir: 3 };

            let bytes = codec.encode(&original).unwrap();
            let env = build_test_envelope(&bytes);
            let archived: &ArchivedMove =
                <RkyvCodec as Decode<Move>>::decode(&codec, &env).unwrap();

            assert_eq!(archived.steps, 42);
            assert_eq!(archived.dir, 3);
        }
    }
}
