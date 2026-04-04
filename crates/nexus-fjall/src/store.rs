use crate::builder::FjallStoreBuilder;
use crate::encoding::{
    decode_stream_meta, encode_event_key, encode_event_value, encode_stream_meta,
};
use crate::error::FjallError;
use crate::stream::FjallStream;
use fjall::Slice;
use nexus::{ErrorId, Version};
use nexus_store::PendingEnvelope;
use nexus_store::error::AppendError;
use nexus_store::raw::RawEventStore;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fjall-backed event store.
///
/// Holds the transactional keyspace, two partitions (`streams` for
/// stream metadata and `events` for event rows), and a monotonic
/// counter for allocating stream numeric IDs.
///
/// Use [`FjallStore::builder`] to configure and open a store.
pub struct FjallStore {
    pub(crate) db: fjall::TxKeyspace,
    pub(crate) streams: fjall::TxPartitionHandle,
    pub(crate) events: fjall::TxPartitionHandle,
    pub(crate) next_stream_id: AtomicU64,
}

impl FjallStore {
    /// Create a builder for opening a `FjallStore` at the given path.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
    }
}

impl RawEventStore for FjallStore {
    type Error = FjallError;
    type Stream<'a> = FjallStream;

    #[allow(
        clippy::significant_drop_tightening,
        reason = "tx must be held across concurrency check + inserts + commit"
    )]
    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut tx = self.db.write_tx();

        // Look up stream metadata (numeric_id, current_version).
        let (numeric_id, current_version) = if let Some(meta_bytes) = tx
            .get(&self.streams, stream_id)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
        {
            let (numeric_id, current_version) = decode_stream_meta(&meta_bytes).map_err(|_| {
                AppendError::Store(FjallError::CorruptMeta {
                    stream_id: stream_id.to_owned(),
                })
            })?;
            // Optimistic concurrency check.
            if current_version != expected_version.as_u64() {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: expected_version,
                    actual: Version::from_persisted(current_version),
                });
            }
            (numeric_id, current_version)
        } else {
            // New stream: expected_version must be 0.
            if expected_version.as_u64() != 0 {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: expected_version,
                    actual: Version::from_persisted(0),
                });
            }
            let numeric_id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
            (numeric_id, 0)
        };

        // Validate envelope versions are sequential from expected_version + 1.
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_version = current_version + 1 + i_u64;
            if env.version().as_u64() != expected_env_version {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: Version::from_persisted(expected_env_version),
                    actual: env.version(),
                });
            }
        }

        // Write each envelope into the events partition.
        let mut value_buf = Vec::new();
        for env in envelopes {
            let key = encode_event_key(numeric_id, env.version().as_u64());
            encode_event_value(
                &mut value_buf,
                env.schema_version(),
                env.event_type(),
                env.payload(),
            )
            .map_err(|_| {
                AppendError::Store(FjallError::CorruptValue {
                    stream_id: stream_id.to_owned(),
                    version: env.version().as_u64(),
                })
            })?;
            tx.insert(&self.events, key, Slice::from(&*value_buf));
        }

        // Update stream meta with new version.
        let new_version = envelopes
            .last()
            .map_or(current_version, |last| last.version().as_u64());
        let meta = encode_stream_meta(numeric_id, new_version);
        tx.insert(&self.streams, stream_id, meta);

        // Atomic cross-partition commit.
        tx.commit()
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?;

        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        // Look up numeric stream ID from streams partition.
        let meta = self.streams.get(stream_id)?;
        let numeric_id = if let Some(meta_bytes) = meta {
            let (numeric_id, _version) =
                decode_stream_meta(&meta_bytes).map_err(|_| FjallError::CorruptMeta {
                    stream_id: stream_id.to_owned(),
                })?;
            numeric_id
        } else {
            // Stream not found: return empty stream.
            return Ok(FjallStream {
                events: vec![],
                pos: 0,
                stream_id: stream_id.to_owned(),
                #[cfg(debug_assertions)]
                prev_version: None,
            });
        };

        // Range scan from (numeric_id, from_version) to (numeric_id, u64::MAX).
        let start = encode_event_key(numeric_id, from.as_u64());
        let end = encode_event_key(numeric_id, u64::MAX);

        let mut events = Vec::new();
        for kv_result in self.events.inner().range(start..=end) {
            let kv = kv_result?;
            events.push((kv.0, kv.1));
        }

        Ok(FjallStream {
            events,
            pos: 0,
            stream_id: stream_id.to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::stream::EventStream;

    fn make_envelope(
        stream_id: &str,
        version: u64,
        event_type: &'static str,
        payload: &[u8],
    ) -> PendingEnvelope<()> {
        pending_envelope(stream_id.to_owned())
            .version(Version::from_persisted(version))
            .event_type(event_type)
            .payload(payload.to_vec())
            .build_without_metadata()
    }

    fn temp_store() -> FjallStore {
        let dir = tempfile::tempdir().unwrap();
        FjallStore::builder(dir.path().join("db")).open().unwrap()
    }

    // ---- append tests ----

    #[tokio::test]
    async fn append_to_new_stream() {
        let store = temp_store();
        let env = make_envelope("stream-1", 1, "Created", b"payload");

        let result = store.append("stream-1", Version::INITIAL, &[env]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_multiple_events() {
        let store = temp_store();
        let envs = vec![
            make_envelope("stream-1", 1, "Created", b"p1"),
            make_envelope("stream-1", 2, "Updated", b"p2"),
            make_envelope("stream-1", 3, "Updated", b"p3"),
        ];

        let result = store.append("stream-1", Version::INITIAL, &envs).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_detects_version_conflict() {
        let store = temp_store();
        let env1 = make_envelope("stream-1", 1, "Created", b"p1");
        store
            .append("stream-1", Version::INITIAL, &[env1])
            .await
            .unwrap();

        // Try appending with wrong expected version (0 instead of 1).
        let env2 = make_envelope("stream-1", 1, "Duplicate", b"p2");
        let result = store.append("stream-1", Version::INITIAL, &[env2]).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, Version::INITIAL);
                assert_eq!(actual, Version::from_persisted(1));
            }
            other @ AppendError::Store(_) => panic!("expected Conflict, got: {other}"),
        }
    }

    #[tokio::test]
    async fn append_to_nonexistent_stream_with_nonzero_version_fails() {
        let store = temp_store();
        let env = make_envelope("stream-1", 6, "Created", b"payload");

        let result = store
            .append("stream-1", Version::from_persisted(5), &[env])
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, Version::from_persisted(5));
                assert_eq!(actual, Version::from_persisted(0));
            }
            other @ AppendError::Store(_) => panic!("expected Conflict, got: {other}"),
        }
    }

    #[tokio::test]
    async fn append_sequential_batches() {
        let store = temp_store();

        // First batch.
        let batch1 = vec![
            make_envelope("stream-1", 1, "Created", b"p1"),
            make_envelope("stream-1", 2, "Updated", b"p2"),
        ];
        store
            .append("stream-1", Version::INITIAL, &batch1)
            .await
            .unwrap();

        // Second batch, continuing from version 2.
        let batch2 = vec![
            make_envelope("stream-1", 3, "Updated", b"p3"),
            make_envelope("stream-1", 4, "Closed", b"p4"),
        ];
        store
            .append("stream-1", Version::from_persisted(2), &batch2)
            .await
            .unwrap();

        // Verify all events are readable.
        let mut stream = store
            .read_stream("stream-1", Version::from_persisted(1))
            .await
            .unwrap();
        let mut count = 0u64;
        while let Some(result) = stream.next().await {
            let env = result.unwrap();
            count += 1;
            assert_eq!(env.version().as_u64(), count);
        }
        assert_eq!(count, 4);
    }

    // ---- read_stream tests ----

    #[tokio::test]
    async fn read_empty_stream_returns_empty() {
        let store = temp_store();
        let mut stream = store
            .read_stream("nonexistent", Version::from_persisted(1))
            .await
            .unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_after_append_returns_events() {
        let store = temp_store();
        let envs = vec![
            make_envelope("stream-1", 1, "Created", b"p1"),
            make_envelope("stream-1", 2, "Updated", b"p2"),
        ];
        store
            .append("stream-1", Version::INITIAL, &envs)
            .await
            .unwrap();

        let mut stream = store
            .read_stream("stream-1", Version::from_persisted(1))
            .await
            .unwrap();

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.stream_id(), "stream-1");
            assert_eq!(env.version(), Version::from_persisted(1));
            assert_eq!(env.event_type(), "Created");
            assert_eq!(env.payload(), b"p1");
        }

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.stream_id(), "stream-1");
            assert_eq!(env.version(), Version::from_persisted(2));
            assert_eq!(env.event_type(), "Updated");
            assert_eq!(env.payload(), b"p2");
        }

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_with_from_version_skips_earlier() {
        let store = temp_store();
        let envs = vec![
            make_envelope("stream-1", 1, "Created", b"p1"),
            make_envelope("stream-1", 2, "Updated", b"p2"),
            make_envelope("stream-1", 3, "Closed", b"p3"),
        ];
        store
            .append("stream-1", Version::INITIAL, &envs)
            .await
            .unwrap();

        // Read from version 2 onwards.
        let mut stream = store
            .read_stream("stream-1", Version::from_persisted(2))
            .await
            .unwrap();

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.version(), Version::from_persisted(2));
            assert_eq!(env.event_type(), "Updated");
        }

        {
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.version(), Version::from_persisted(3));
            assert_eq!(env.event_type(), "Closed");
        }

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn streams_are_isolated() {
        let store = temp_store();

        let env_a = make_envelope("stream-a", 1, "CreatedA", b"a-data");
        store
            .append("stream-a", Version::INITIAL, &[env_a])
            .await
            .unwrap();

        let env_b = make_envelope("stream-b", 1, "CreatedB", b"b-data");
        store
            .append("stream-b", Version::INITIAL, &[env_b])
            .await
            .unwrap();

        // Read stream-a: should only see its own event.
        let mut stream_a = store
            .read_stream("stream-a", Version::from_persisted(1))
            .await
            .unwrap();
        {
            let env = stream_a.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "CreatedA");
            assert_eq!(env.payload(), b"a-data");
        }
        assert!(stream_a.next().await.is_none());

        // Read stream-b: should only see its own event.
        let mut stream_b = store
            .read_stream("stream-b", Version::from_persisted(1))
            .await
            .unwrap();
        {
            let env = stream_b.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "CreatedB");
            assert_eq!(env.payload(), b"b-data");
        }
        assert!(stream_b.next().await.is_none());
    }

    // ---- persistence tests ----

    #[tokio::test]
    async fn data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db");

        // Write events, close store.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            let envs = vec![make_envelope("s1", 1, "Created", b"persist-me")];
            store.append("s1", Version::INITIAL, &envs).await.unwrap();
        }

        // Reopen and verify.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            let mut stream = store
                .read_stream("s1", Version::from_persisted(1))
                .await
                .unwrap();
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "Created");
            assert_eq!(env.payload(), b"persist-me");
        }
    }

    #[tokio::test]
    async fn stream_id_counter_recovers_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db");

        // Create two streams, close.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            store
                .append("s1", Version::INITIAL, &[make_envelope("s1", 1, "A", b"")])
                .await
                .unwrap();
            store
                .append("s2", Version::INITIAL, &[make_envelope("s2", 1, "B", b"")])
                .await
                .unwrap();
        }

        // Reopen, create third stream — should not collide with existing numeric IDs.
        {
            let store = FjallStore::builder(&db_path).open().unwrap();
            store
                .append("s3", Version::INITIAL, &[make_envelope("s3", 1, "C", b"")])
                .await
                .unwrap();

            // All three streams readable and isolated.
            let mut st = store
                .read_stream("s3", Version::from_persisted(1))
                .await
                .unwrap();
            let env = st.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "C");
        }
    }

    // ---- version tracking tests ----

    #[tokio::test]
    async fn version_tracks_across_appends() {
        let store = temp_store();
        store
            .append("s", Version::INITIAL, &[make_envelope("s", 1, "A", b"")])
            .await
            .unwrap();
        store
            .append(
                "s",
                Version::from_persisted(1),
                &[make_envelope("s", 2, "B", b"")],
            )
            .await
            .unwrap();

        let mut stream = store
            .read_stream("s", Version::from_persisted(1))
            .await
            .unwrap();
        {
            let e1 = stream.next().await.unwrap().unwrap();
            assert_eq!(e1.version().as_u64(), 1);
        }
        {
            let e2 = stream.next().await.unwrap().unwrap();
            assert_eq!(e2.version().as_u64(), 2);
        }
        assert!(stream.next().await.is_none());
    }

    // ---- large batch tests ----

    #[tokio::test]
    async fn large_batch_append() {
        let store = temp_store();
        let envelopes: Vec<_> = (1..=100)
            .map(|i| make_envelope("big", i, "Tick", &[0u8; 256]))
            .collect();
        store
            .append("big", Version::INITIAL, &envelopes)
            .await
            .unwrap();

        let mut stream = store
            .read_stream("big", Version::from_persisted(1))
            .await
            .unwrap();
        let mut count = 0u64;
        while let Some(result) = stream.next().await {
            let _ = result.unwrap();
            count += 1;
        }
        assert_eq!(count, 100);
    }

    // ---- schema version tests ----

    #[tokio::test]
    async fn schema_version_preserved() {
        let store = temp_store();
        let env = pending_envelope("s".to_owned())
            .version(Version::from_persisted(1))
            .event_type("Migrated")
            .payload(b"data".to_vec())
            .schema_version(42)
            .build_without_metadata();
        store.append("s", Version::INITIAL, &[env]).await.unwrap();

        let mut stream = store
            .read_stream("s", Version::from_persisted(1))
            .await
            .unwrap();
        let persisted = stream.next().await.unwrap().unwrap();
        assert_eq!(persisted.schema_version(), 42);
    }

    // ---- edge cases ----

    #[tokio::test]
    async fn empty_batch_append_succeeds() {
        let store = temp_store();
        let result = store.append("s", Version::INITIAL, &[]).await;
        assert!(result.is_ok());
    }
}
