use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Row stored per event: (version, `event_type`, payload).
type StoredRow = (u64, String, Vec<u8>);

/// Minimal in-memory adapter for testing `RawEventStore`.
struct InMemoryRawStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemoryRawStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

struct InMemoryStream {
    events: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl EventStream for InMemoryStream {
    type Error = TestError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            &row.3,
            (),
        )))
    }
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("concurrency conflict")]
    Conflict,
}

impl RawEventStore for InMemoryRawStore {
    type Error = TestError;
    type Stream<'a>
        = InMemoryStream
    where
        Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_version != expected_version.as_u64() {
            return Err(TestError::Conflict);
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        drop(guard);
        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|(v, _, _)| *v >= from.as_u64())
                    .map(|(v, t, p)| (stream_id.to_owned(), *v, t.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(InMemoryStream { events, pos: 0 })
    }
}

#[tokio::test]
async fn raw_store_append_and_read() {
    let store = InMemoryRawStore::new();

    let envelopes = vec![
        PendingEnvelope::new(
            "s1".into(),
            Version::from_persisted(1),
            "Created",
            vec![1],
            (),
        ),
        PendingEnvelope::new(
            "s1".into(),
            Version::from_persisted(2),
            "Updated",
            vec![2],
            (),
        ),
    ];

    store
        .append("s1", Version::INITIAL, &envelopes)
        .await
        .unwrap();

    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();

    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.event_type(), "Created");
    }

    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.event_type(), "Updated");
    }

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn raw_store_optimistic_concurrency() {
    let store = InMemoryRawStore::new();

    let e1 = vec![PendingEnvelope::new(
        "s1".into(),
        Version::from_persisted(1),
        "E",
        vec![],
        (),
    )];
    store.append("s1", Version::INITIAL, &e1).await.unwrap();

    // Wrong expected version -- should fail.
    let e2 = vec![PendingEnvelope::new(
        "s1".into(),
        Version::from_persisted(2),
        "E",
        vec![],
        (),
    )];
    let result = store.append("s1", Version::INITIAL, &e2).await;
    assert!(result.is_err());
}
