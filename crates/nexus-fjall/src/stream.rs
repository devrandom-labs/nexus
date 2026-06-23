use std::collections::VecDeque;

use arrayvec::ArrayString;
use bytes::Bytes;
use fjall::{SingleWriterTxKeyspace, Slice};
use nexus::Version;
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;

use crate::encoding::{decode_event_key, decode_event_value, encode_event_key};
use crate::error::{FjallError, reason_label};
use crate::subscription_id::OwnedStreamId;

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
/// Created by `FjallStore::read_stream`. Events are loaded in bounded batches
/// (at most `batch_size` rows at a time) via keyset pagination, so the
/// in-memory footprint is bounded regardless of stream length.
pub struct FjallStream {
    /// Current in-memory batch, drained front-to-back.
    events: VecDeque<(Slice, Slice)>,
    /// Cloned `events` partition handle — Arc-backed, cheap to hold; used to
    /// scan the next batch when `events` drains.
    keyspace: SingleWriterTxKeyspace,
    /// Stream key bytes for refill key encoding.
    id: OwnedStreamId,
    /// Next version to scan from on the following refill (keyset cursor).
    next_version: u64,
    /// Max rows per batch.
    batch_size: usize,
    /// Set once a refill returns fewer than `batch_size` rows — no more data.
    done: bool,
    stream_id: ArrayString<64>,
    /// Once an error is yielded, the stream is poisoned — subsequent polls
    /// return `None` rather than silently skipping corrupt entries.
    poisoned: bool,
    #[cfg(debug_assertions)]
    prev_version: Option<u64>,
}

impl FjallStream {
    /// Build a stream and eagerly load the first batch (so the first poll is
    /// IO-free), scanning from `from`.
    pub(crate) fn new(
        keyspace: SingleWriterTxKeyspace,
        id: OwnedStreamId,
        from: u64,
        batch_size: usize,
        stream_id: ArrayString<64>,
    ) -> Result<Self, FjallError> {
        let mut stream = Self {
            events: VecDeque::new(),
            keyspace,
            id,
            next_version: from,
            batch_size,
            done: false,
            stream_id,
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };
        stream.refill()?;
        Ok(stream)
    }

    /// Scan the next `batch_size` rows from `next_version` into `events`.
    /// A short batch (fewer than `batch_size`) sets `done`.
    fn refill(&mut self) -> Result<(), FjallError> {
        let id_bytes = self.id.as_ref();
        let start = encode_event_key(id_bytes, self.next_version).map_err(|e| {
            FjallError::InvalidInput {
                stream_id: self.stream_id,
                version: self.next_version,
                reason: reason_label(&e),
            }
        })?;
        let end = encode_event_key(id_bytes, u64::MAX).map_err(|e| FjallError::InvalidInput {
            stream_id: self.stream_id,
            version: u64::MAX,
            reason: reason_label(&e),
        })?;

        let batch: Vec<(Slice, Slice)> = self
            .keyspace
            .inner()
            .range(start..=end)
            .take(self.batch_size)
            .map(fjall::Guard::into_inner)
            .collect::<Result<_, _>>()?;

        self.done = batch.len() < self.batch_size;
        self.events = batch.into();
        Ok(())
    }

    /// Returns `true` if the current batch buffer is empty.
    ///
    /// Used by [`crate::subscription_stream`] to decide whether to sleep on the
    /// per-stream `Notify` after a refill that found no new events.
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        loop {
            if let Some((key, value)) = self.events.pop_front() {
                return Some(match decode_row(&key, value, self.stream_id) {
                    Ok(env) => {
                        let v = env.version().as_u64();
                        #[cfg(debug_assertions)]
                        {
                            if let Some(prev) = self.prev_version {
                                debug_assert!(
                                    v > prev,
                                    "EventStream monotonicity violated: version {v} is not greater than previous {prev}",
                                );
                            }
                            self.prev_version = Some(v);
                        }
                        // Advance keyset cursor. No event can follow u64::MAX.
                        if let Some(n) = v.checked_add(1) {
                            self.next_version = n;
                        } else {
                            self.done = true;
                            self.next_version = v;
                        }
                        Ok(env)
                    }
                    Err(e) => {
                        self.poisoned = true;
                        Err(e)
                    }
                });
            }
            if self.done {
                return None;
            }
            if let Err(e) = self.refill() {
                self.poisoned = true;
                return Some(Err(e));
            }
            if self.events.is_empty() {
                return None;
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
