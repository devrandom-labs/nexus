use std::collections::VecDeque;

use arrayvec::ArrayString;
use bytes::Bytes;
use fjall::Slice;
use nexus::Version;
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;

use crate::encoding::{decode_event_key, decode_event_value};
use crate::error::FjallError;

/// Decode one stored `(key, value)` row into a [`PersistedEnvelope`].
///
/// Pure: no store handle, no cursor state — so unit tests can exercise it
/// with hand-built rows. Maps every malformed shape to a typed `FjallError`.
fn decode_row(
    key: &Slice,
    value: Slice,
    stream_id: ArrayString<64>,
) -> Result<PersistedEnvelope, FjallError> {
    let (_id_bytes, version) = decode_event_key(key).map_err(|_| FjallError::CorruptValue {
        stream_id,
        version: None,
    })?;

    let bytes_value: Bytes = value.into();
    let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    let ver = Version::new(version).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    let global_seq = GlobalSeq::new(decoded.global_seq).ok_or(FjallError::CorruptValue {
        stream_id,
        version: Some(version),
    })?;

    PersistedEnvelope::try_new(
        ver,
        global_seq,
        bytes_value,
        decoded.schema_version,
        decoded.event_type_range,
        decoded.payload_range,
        decoded.metadata_range,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id,
        version,
        source,
    })
}

/// `futures::Stream` of fjall event rows.
///
/// Created by `FjallStore::read_stream`. Events are eagerly loaded into a
/// `VecDeque` (range scan over the events partition) at construction so
/// `poll_next` is pure sync over the queue — no LSM-tree iterator stays
/// open while the stream is held.
pub struct FjallStream {
    pub(crate) events: VecDeque<(Slice, Slice)>,
    pub(crate) stream_id: ArrayString<64>,
    /// Once an error is yielded, the stream is poisoned — all subsequent
    /// `poll_next` calls return `None`. This prevents silently skipping
    /// corrupt entries on retry.
    pub(crate) poisoned: bool,
    #[cfg(debug_assertions)]
    pub(crate) prev_version: Option<u64>,
}

impl FjallStream {
    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        let (key, value) = self.events.pop_front()?;
        match decode_row(&key, value, self.stream_id) {
            Ok(env) => {
                #[cfg(debug_assertions)]
                {
                    let v = env.version().as_u64();
                    if let Some(prev) = self.prev_version {
                        debug_assert!(
                            v > prev,
                            "EventStream monotonicity violated: version {v} is not greater than previous {prev}",
                        );
                    }
                    self.prev_version = Some(v);
                }
                Some(Ok(env))
            }
            Err(e) => {
                self.poisoned = true;
                Some(Err(e))
            }
        }
    }
}

impl futures::Stream for FjallStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.poll_one())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use crate::encoding::{encode_event_key, encode_event_value};

    fn row(id: &[u8], version: u64, global_seq: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_event_key(id, version).unwrap();
        let mut val = Vec::new();
        encode_event_value(&mut val, global_seq, 1, et, None, payload).unwrap();
        (Slice::from(key), Slice::from(val))
    }

    #[test]
    fn decode_row_yields_envelope() {
        let (k, v) = row(b"user-1", 7, 42, "Created", b"data");
        let sid = ArrayString::try_from("user-1").unwrap();
        let env = decode_row(&k, v, sid).unwrap();
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn decode_row_rejects_truncated_value() {
        let k = Slice::from(encode_event_key(b"corrupt", 1).unwrap());
        let v = Slice::from(&[0u8, 1, 2][..]);
        let sid = ArrayString::try_from("corrupt").unwrap();
        match decode_row(&k, v, sid).unwrap_err() {
            FjallError::CorruptValue { stream_id, version } => {
                assert_eq!(stream_id.as_str(), "corrupt");
                assert_eq!(version, Some(1));
            }
            other => panic!("expected CorruptValue, got {other:?}"),
        }
    }
}
