use std::collections::VecDeque;

use bytes::Bytes;
use fjall::{SingleWriterTxKeyspace, Slice};
use nexus::Version;
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;

use crate::encoding::{decode_event_value, decode_global_key, encode_global_key};
use crate::error::FjallError;

/// Decode one `events_global` `(key, value)` row into a [`PersistedEnvelope`].
///
/// The `global_seq` in the key and in the frame value are validated to match
/// (CLAUDE.md rule 4 — redundant data must be validated). The value is the
/// identical wire frame stored in the primary `events` partition, so it flows
/// zero-copy into the envelope.
fn decode_global_row(key: &Slice, value: Slice) -> Result<PersistedEnvelope, FjallError> {
    let (key_global_seq, version_raw) =
        decode_global_key(key).map_err(|_| FjallError::CorruptValue {
            stream_id: arrayvec::ArrayString::new(),
            version: None,
        })?;

    let bytes_value: Bytes = value.into();
    let decoded = decode_event_value(&bytes_value).map_err(|_| FjallError::CorruptValue {
        stream_id: arrayvec::ArrayString::new(),
        version: Some(version_raw),
    })?;

    if decoded.global_seq != key_global_seq {
        return Err(FjallError::CorruptValue {
            stream_id: arrayvec::ArrayString::new(),
            version: Some(version_raw),
        });
    }

    let version = Version::new(version_raw).ok_or_else(|| FjallError::CorruptValue {
        stream_id: arrayvec::ArrayString::new(),
        version: Some(version_raw),
    })?;
    let global_seq =
        GlobalSeq::new(decoded.global_seq).ok_or_else(|| FjallError::CorruptValue {
            stream_id: arrayvec::ArrayString::new(),
            version: Some(version_raw),
        })?;

    PersistedEnvelope::try_new(
        version,
        global_seq,
        bytes_value,
        decoded.schema_version,
        decoded.event_type_range,
        decoded.payload_range,
        decoded.metadata_range,
    )
    .map_err(|source| FjallError::EnvelopeCorrupt {
        stream_id: arrayvec::ArrayString::new(),
        version: version_raw,
        source,
    })
}

/// `futures::Stream` of fjall events in `GlobalSeq` order (the `$all` read).
///
/// Created by `FjallStore::read_all`. Loads at most `batch_size` rows per
/// batch via keyset pagination on `global_seq`, so memory is bounded
/// regardless of how many events exist.
pub struct FjallAllStream {
    events: VecDeque<(Slice, Slice)>,
    keyspace: SingleWriterTxKeyspace,
    /// Next `global_seq` to scan from (inclusive).
    next_global_seq: u64,
    batch_size: usize,
    done: bool,
    poisoned: bool,
}

impl FjallAllStream {
    /// Build and eagerly load the first batch, scanning from `from` (inclusive).
    pub(crate) fn new(
        keyspace: SingleWriterTxKeyspace,
        from: u64,
        batch_size: usize,
    ) -> Result<Self, FjallError> {
        let mut stream = Self {
            events: VecDeque::new(),
            keyspace,
            next_global_seq: from,
            batch_size,
            done: false,
            poisoned: false,
        };
        stream.refill()?;
        Ok(stream)
    }

    fn refill(&mut self) -> Result<(), FjallError> {
        // [global_seq=from][version=0] .. [global_seq=MAX][version=MAX].
        // global_seq is the high-order 8 bytes, so this spans every event with
        // global_seq >= next_global_seq across all streams.
        let start = encode_global_key(self.next_global_seq, 0);
        let end = encode_global_key(u64::MAX, u64::MAX);

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
    /// Used by the `$all` subscription cursor (Phase 2) to decide whether to
    /// park on the store-wide `Notify` after a refill that found no new events.
    #[allow(dead_code, reason = "used by the Phase 2 all-subscription cursor")]
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn poll_one(&mut self) -> Option<Result<PersistedEnvelope, FjallError>> {
        if self.poisoned {
            return None;
        }
        loop {
            if let Some((key, value)) = self.events.pop_front() {
                return Some(match decode_global_row(&key, value) {
                    Ok(env) => {
                        // Resume strictly after this global_seq.
                        match env.global_seq().as_u64().checked_add(1) {
                            Some(n) => self.next_global_seq = n,
                            None => self.done = true,
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

impl futures::Stream for FjallAllStream {
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
    use crate::encoding::encode_event_value;

    fn global_row(global_seq: u64, version: u64, et: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_global_key(global_seq, version);
        let mut val = Vec::new();
        encode_event_value(&mut val, global_seq, 1, et, None, payload).unwrap();
        (Slice::from(&key[..]), Slice::from(val))
    }

    #[test]
    fn decode_global_row_yields_envelope() {
        let (k, v) = global_row(42, 7, "Created", b"data");
        let env = decode_global_row(&k, v).unwrap();
        assert_eq!(env.global_seq(), GlobalSeq::new(42).unwrap());
        assert_eq!(env.version(), Version::new(7).unwrap());
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"data");
    }

    #[test]
    fn decode_global_row_rejects_global_seq_mismatch() {
        // Key says global_seq 99, value frame says 42 → corruption.
        let key = encode_global_key(99, 7);
        let mut val = Vec::new();
        encode_event_value(&mut val, 42, 1, "E", None, b"x").unwrap();
        let err = decode_global_row(&Slice::from(&key[..]), Slice::from(val)).unwrap_err();
        assert!(matches!(err, FjallError::CorruptValue { .. }));
    }
}
