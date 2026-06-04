use std::collections::VecDeque;

use arrayvec::ArrayString;
use bytes::Bytes;
use fjall::Slice;
use nexus::Version;
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;

use crate::encoding::{decode_event_key, decode_event_value};
use crate::error::FjallError;

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

        let Ok((_id_bytes, version)) = decode_event_key(&key) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id,
                version: None,
            }));
        };

        #[cfg(debug_assertions)]
        {
            if let Some(prev) = self.prev_version {
                debug_assert!(
                    version > prev,
                    "EventStream monotonicity violated: version {version} \
                     is not greater than previous {prev}",
                );
            }
            self.prev_version = Some(version);
        }

        let bytes_value: Bytes = value.into();
        let Ok(decoded) = decode_event_value(&bytes_value) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id,
                version: Some(version),
            }));
        };

        let Some(ver) = Version::new(version) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id,
                version: Some(version),
            }));
        };

        let Some(global_seq) = GlobalSeq::new(decoded.global_seq) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id,
                version: Some(version),
            }));
        };

        match PersistedEnvelope::try_new(
            ver,
            global_seq,
            bytes_value,
            decoded.schema_version,
            decoded.event_type_range,
            decoded.payload_range,
            decoded.metadata_range,
        ) {
            Ok(envelope) => Some(Ok(envelope)),
            Err(source) => {
                self.poisoned = true;
                Some(Err(FjallError::EnvelopeCorrupt {
                    stream_id: self.stream_id,
                    version,
                    source,
                }))
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
#[allow(clippy::too_many_arguments, reason = "test fixture helper")]
mod tests {
    use super::*;
    use crate::encoding::{encode_event_key, encode_event_value};
    use futures::StreamExt;

    fn make_row(
        id: &[u8],
        version: u64,
        global_seq: u64,
        schema_ver: u32,
        event_type: &str,
        payload: &[u8],
    ) -> (Slice, Slice) {
        let key = encode_event_key(id, version).unwrap();
        let mut val_buf = Vec::new();
        encode_event_value(
            &mut val_buf,
            global_seq,
            schema_ver,
            event_type,
            None,
            payload,
        )
        .unwrap();
        (Slice::from(key), Slice::from(val_buf))
    }

    #[tokio::test]
    async fn yields_events_in_order() {
        let row1 = make_row(b"user-123", 1, 10, 1, "UserCreated", b"payload-1");
        let row2 = make_row(b"user-123", 2, 11, 1, "UserUpdated", b"payload-2");

        let mut stream = FjallStream {
            events: VecDeque::from(vec![row1, row2]),
            stream_id: ArrayString::try_from("user-123").unwrap(),
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        {
            let env1 = stream.next().await.unwrap().unwrap();
            assert_eq!(env1.version(), Version::new(1).unwrap());
            assert_eq!(env1.global_seq(), GlobalSeq::new(10).unwrap());
            assert_eq!(env1.event_type(), "UserCreated");
            assert_eq!(env1.schema_version(), 1);
            assert_eq!(env1.payload(), b"payload-1");
        }

        {
            let env2 = stream.next().await.unwrap().unwrap();
            assert_eq!(env2.version(), Version::new(2).unwrap());
            assert_eq!(env2.global_seq(), GlobalSeq::new(11).unwrap());
            assert_eq!(env2.event_type(), "UserUpdated");
            assert_eq!(env2.schema_version(), 1);
            assert_eq!(env2.payload(), b"payload-2");
        }
    }

    #[tokio::test]
    async fn corrupt_value_returns_error() {
        // Valid key, but truncated value (only 3 bytes, header requires 18).
        let key = encode_event_key(b"corrupt-stream", 1).unwrap();
        let truncated_value: &[u8] = &[0, 1, 2];

        let mut stream = FjallStream {
            events: VecDeque::from(vec![(Slice::from(key), Slice::from(truncated_value))]),
            stream_id: ArrayString::try_from("corrupt-stream").unwrap(),
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        let err = stream.next().await.unwrap().unwrap_err();
        match err {
            FjallError::CorruptValue { stream_id, version } => {
                assert_eq!(stream_id.as_str(), "corrupt-stream");
                assert_eq!(version, Some(1));
            }
            other => panic!("expected CorruptValue, got: {other:?}"),
        }
    }
}
