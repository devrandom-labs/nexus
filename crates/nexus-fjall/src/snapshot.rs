//! Snapshot value codec (feature `snapshot`).
//!
//! The snapshot blob layout is fjall-private and distinct from the event
//! wire format: `[u32 LE schema_version][u64 BE version][payload]`. Used only
//! by the `SnapshotStore<Vec<u8>, Version>` impl in [`crate::store`].

use crate::wire_key::DecodeError;

/// Size of the snapshot value header: `[u32 LE schema_version][u64 BE version]`.
pub(crate) const SNAPSHOT_VALUE_HEADER_SIZE: usize = 12;

/// Encode a snapshot value as `[u32 LE schema_version][u64 BE version][payload]`.
pub(crate) fn encode_snapshot_value(
    buf: &mut Vec<u8>,
    schema_version: u32,
    version: u64,
    payload: &[u8],
) {
    buf.clear();
    buf.reserve(SNAPSHOT_VALUE_HEADER_SIZE + payload.len());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&version.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// Decode a snapshot value from `[u32 LE schema_version][u64 BE version][payload]`.
///
/// # Errors
///
/// Returns [`DecodeError::ValueTooShort`] if value is shorter than the header.
pub(crate) fn decode_snapshot_value(value: &[u8]) -> Result<(u32, u64, &[u8]), DecodeError> {
    if value.len() < SNAPSHOT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: SNAPSHOT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }
    let schema_version = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let version = u64::from_be_bytes([
        value[4], value[5], value[6], value[7], value[8], value[9], value[10], value[11],
    ]);
    let payload = &value[12..];
    Ok((schema_version, version, payload))
}
