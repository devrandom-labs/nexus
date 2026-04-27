#[cfg(feature = "json")]
pub mod json;

use ::serde::{Serialize, de::DeserializeOwned};

use crate::Codec;

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
