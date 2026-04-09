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
