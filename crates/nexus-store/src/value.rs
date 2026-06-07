//! Validated value newtypes for envelope fields.
//!
//! Each newtype owns the wire-format invariants for one envelope field:
//! UTF-8 validity (where applicable) and the size cap dictated by the
//! wire format's length-prefix field. Once a value is constructed, it
//! is by definition wire-encodable; downstream layers (`wire.rs`,
//! adapters) skip re-validation.
//!
//! Backed by `bytes::Bytes` for cheap Arc-shared ownership. The
//! `Bytes::from_static` path makes literal event-type names
//! allocation-free, matching the previous `&'static str` ergonomics.

use thiserror::Error;

/// Maximum event-type length (the wire format reserves a `u16` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u16→usize widening; lossless on all targets"
)]
pub const MAX_EVENT_TYPE_LEN: usize = u16::MAX as usize;

/// Maximum metadata length. One less than `u32::MAX` because the wire
/// format uses `u32::MAX` as the absent-metadata sentinel.
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_METADATA_LEN: usize = (u32::MAX - 1) as usize;

/// Maximum payload length (the wire format reserves a `u32` length field).
#[allow(
    clippy::as_conversions,
    reason = "const-context u32→usize widening; lossless on 32+ bit targets"
)]
pub const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// Construction errors for value newtypes.
#[derive(Debug, Error)]
pub enum ValueError {
    #[error("event_type length {actual} exceeds maximum {MAX_EVENT_TYPE_LEN}")]
    EventTypeTooLong { actual: usize },
    #[error("invalid UTF-8 in event_type bytes (at byte {valid_up_to})")]
    EventTypeInvalidUtf8 {
        valid_up_to: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("metadata length {actual} exceeds maximum {MAX_METADATA_LEN}")]
    MetadataTooLong { actual: usize },
    #[error("payload length {actual} exceeds maximum {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { actual: usize },
}
