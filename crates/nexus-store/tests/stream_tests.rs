use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

/// In-memory test stream that yields from a Vec of owned data.
struct VecStream {
    rows: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl VecStream {
    fn new(rows: Vec<(String, u64, String, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl EventStream for VecStream {
    type Error = std::convert::Infallible;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.rows.len() {
            return None;
        }
        let row = &self.rows[self.pos];
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

#[tokio::test]
async fn event_stream_yields_envelopes() {
    let mut stream = VecStream::new(vec![
        ("s1".into(), 1, "Created".into(), vec![1]),
        ("s1".into(), 2, "Updated".into(), vec![2]),
    ]);

    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.stream_id(), "s1");
    assert_eq!(e1.version(), Version::from_persisted(1));
    assert_eq!(e1.event_type(), "Created");
    drop(e1);

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.version(), Version::from_persisted(2));
    assert_eq!(e2.event_type(), "Updated");
    drop(e2);

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn event_stream_empty() {
    let mut stream = VecStream::new(vec![]);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn event_stream_envelope_borrows_from_cursor() {
    let mut stream = VecStream::new(vec![("stream".into(), 1, "Event".into(), vec![42])]);

    let envelope = stream.next().await.unwrap().unwrap();
    assert_eq!(envelope.payload(), &[42]);
    drop(envelope);

    assert!(stream.next().await.is_none());
}
