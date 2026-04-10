use crate::encoding::{decode_event_key, decode_event_value};
use crate::error::FjallError;
use fjall::Slice;
use nexus::Version;
use nexus_store::PersistedEnvelope;
use nexus_store::store::EventStream;

/// Lending cursor over fjall event rows.
///
/// Created by `FjallStore::read_stream`. The events are eagerly loaded into
/// a `Vec` (range scan over the events partition) so the cursor can lend
/// references into the buffer without holding an LSM-tree iterator open.
pub struct FjallStream {
    pub(crate) events: Vec<(Slice, Slice)>,
    pub(crate) pos: usize,
    pub(crate) stream_id: String,
    /// Once an error is returned, the stream is poisoned — all subsequent
    /// calls to `next()` return `None`. This prevents silently skipping
    /// corrupt entries on retry.
    pub(crate) poisoned: bool,
    #[cfg(debug_assertions)]
    pub(crate) prev_version: Option<u64>,
}

impl EventStream for FjallStream {
    type Error = FjallError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.poisoned || self.pos >= self.events.len() {
            return None;
        }

        let (key, value) = &self.events[self.pos];
        self.pos += 1;

        let Ok((_stream_num, version)) = decode_event_key(key) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id.clone(),
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

        let Ok((schema_version, event_type, payload)) = decode_event_value(value) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id.clone(),
                version: Some(version),
            }));
        };

        let Some(ver) = Version::new(version) else {
            self.poisoned = true;
            return Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id.clone(),
                version: Some(version),
            }));
        };

        if let Ok(envelope) =
            PersistedEnvelope::try_new(ver, event_type, schema_version, payload, ())
        {
            Some(Ok(envelope))
        } else {
            self.poisoned = true;
            Some(Err(FjallError::CorruptValue {
                stream_id: self.stream_id.clone(),
                version: Some(version),
            }))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use crate::encoding::{encode_event_key, encode_event_value};

    fn make_row(
        stream_num: u64,
        version: u64,
        schema_ver: u32,
        event_type: &str,
        payload: &[u8],
    ) -> (Slice, Slice) {
        let key = encode_event_key(stream_num, version);
        let mut val_buf = Vec::new();
        encode_event_value(&mut val_buf, schema_ver, event_type, payload).unwrap();
        (Slice::from(key), Slice::from(val_buf))
    }

    #[tokio::test]
    async fn yields_events_in_order() {
        let row1 = make_row(1, 1, 1, "UserCreated", b"payload-1");
        let row2 = make_row(1, 2, 1, "UserUpdated", b"payload-2");

        let mut stream = FjallStream {
            events: vec![row1, row2],
            pos: 0,
            stream_id: "user-123".into(),
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        {
            let env1 = stream.next().await.unwrap().unwrap();
            assert_eq!(env1.version(), Version::new(1).unwrap());
            assert_eq!(env1.event_type(), "UserCreated");
            assert_eq!(env1.schema_version(), 1);
            assert_eq!(env1.payload(), b"payload-1");
        }

        {
            let env2 = stream.next().await.unwrap().unwrap();
            assert_eq!(env2.version(), Version::new(2).unwrap());
            assert_eq!(env2.event_type(), "UserUpdated");
            assert_eq!(env2.schema_version(), 1);
            assert_eq!(env2.payload(), b"payload-2");
        }
    }

    #[tokio::test]
    async fn corrupt_value_returns_error() {
        // Valid key, but truncated value (only 3 bytes, header requires 6).
        let key = encode_event_key(1, 1);
        let truncated_value: &[u8] = &[0, 1, 2];

        let mut stream = FjallStream {
            events: vec![(Slice::from(key), Slice::from(truncated_value))],
            pos: 0,
            stream_id: "corrupt-stream".into(),
            poisoned: false,
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        let result = stream.next().await.unwrap();
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            FjallError::CorruptValue { stream_id, version } => {
                assert_eq!(stream_id, "corrupt-stream");
                assert_eq!(version, Some(1));
            }
            other => panic!("expected CorruptValue, got: {other:?}"),
        }
    }
}
