use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

/// In-memory test stream that yields from a Vec of owned data.
struct VecStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
}

impl VecStream {
    const fn new(rows: Vec<(u64, String, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl EventStream for VecStream {
    type Error = std::convert::Infallible;

    #[allow(clippy::expect_used, reason = "test data always has non-zero versions")]
    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("test version must be non-zero"),
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

#[tokio::test]
async fn event_stream_yields_envelopes() {
    let mut stream = VecStream::new(vec![
        (1, "Created".into(), vec![1]),
        (2, "Updated".into(), vec![2]),
    ]);

    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version(), Version::new(1).unwrap());
        assert_eq!(e1.event_type(), "Created");
    }

    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version(), Version::new(2).unwrap());
        assert_eq!(e2.event_type(), "Updated");
    }

    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_empty() {
    let mut stream = VecStream::new(vec![]);
    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_envelope_borrows_from_cursor() {
    let mut stream = VecStream::new(vec![(1, "Event".into(), vec![42])]);

    {
        let envelope = stream.next().await.unwrap().unwrap();
        assert_eq!(envelope.payload(), &[42]);
    }

    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_fused_after_none() {
    let mut stream = VecStream::new(vec![(1, "E".into(), vec![])]);

    // Consume the one event
    let _ = stream.next().await.unwrap().unwrap();

    // First None
    assert!(stream.next().await.unwrap().is_none());
    // Calling again must also return None (fused)
    assert!(stream.next().await.unwrap().is_none());
    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_single_event() {
    let mut stream = VecStream::new(vec![(1, "OnlyEvent".into(), vec![99])]);
    {
        let env = stream.next().await.unwrap().unwrap();
        assert_eq!(env.event_type(), "OnlyEvent");
        assert_eq!(env.payload(), &[99]);
    }
    assert!(stream.next().await.unwrap().is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// Decoder builder + DecodedStream / BorrowedDecodedStream
// ═══════════════════════════════════════════════════════════════════════════

use nexus_store::codec::{BorrowingCodec, Codec};
use nexus_store::error::DecodeStreamError;
use nexus_store::error::UpcastError;
use nexus_store::stream::EventStreamExt;
use nexus_store::upcasting::{EventMorsel, Upcaster};

/// Owning codec for `u64` payloads (little-endian).
struct U64Codec;

#[derive(Debug, thiserror::Error)]
#[error("u64 decode failed: payload had {0} bytes, expected 8")]
struct U64DecodeError(usize);

impl Codec<u64> for U64Codec {
    type Error = U64DecodeError;

    fn encode(&self, value: &u64) -> Result<Vec<u8>, Self::Error> {
        Ok(value.to_le_bytes().to_vec())
    }

    fn decode(&self, _name: &str, payload: &[u8]) -> Result<u64, Self::Error> {
        let bytes: [u8; 8] = payload
            .try_into()
            .map_err(|_| U64DecodeError(payload.len()))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// Borrowing codec that returns `&[u8]` directly from the payload (zero-copy).
struct BytesBorrowingCodec;

#[derive(Debug, thiserror::Error)]
#[error("bytes borrow failed")]
struct BytesBorrowError;

impl BorrowingCodec<[u8]> for BytesBorrowingCodec {
    type Error = BytesBorrowError;

    fn encode(&self, value: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(value.to_vec())
    }

    fn decode<'a>(&self, _name: &str, payload: &'a [u8]) -> Result<&'a [u8], Self::Error> {
        Ok(payload)
    }
}

/// Upcaster that prepends a 1 byte to every payload to verify the upcast path runs.
struct PrependOne;

#[derive(Debug, thiserror::Error)]
#[error("prepend transform failed")]
struct PrependError;

impl Upcaster for PrependOne {
    type Error = PrependError;

    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        let mut new_payload = Vec::with_capacity(morsel.payload().len() + 1);
        new_payload.push(1);
        new_payload.extend_from_slice(morsel.payload());
        let version = nexus::Version::new(morsel.schema_version().as_u64() + 1).unwrap();
        Ok(morsel
            .with_payload(std::borrow::Cow::Owned(new_payload))
            .with_schema_version(version))
    }

    fn current_version(&self, _event_type: &str) -> Option<nexus::Version> {
        nexus::Version::new(2)
    }
}

#[tokio::test]
async fn decoder_owning_no_upcaster_folds_to_sum() {
    let stream = VecStream::new(vec![
        (1, "Tick".into(), 10u64.to_le_bytes().to_vec()),
        (2, "Tick".into(), 20u64.to_le_bytes().to_vec()),
        (3, "Tick".into(), 12u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;

    let total = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            0u64,
            |sum, version, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> {
                // Smoke-check that version threads through and is monotonic.
                assert!(version.as_u64() >= 1);
                Ok(sum + e)
            },
        )
        .await
        .unwrap();

    assert_eq!(total, 42);
}

#[tokio::test]
async fn decoder_owning_passes_version_strict_monotonic() {
    let stream = VecStream::new(vec![
        (1, "E".into(), 0u64.to_le_bytes().to_vec()),
        (2, "E".into(), 0u64.to_le_bytes().to_vec()),
        (3, "E".into(), 0u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;

    let versions: Vec<u64> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            Vec::<u64>::new(),
            |mut v, version, _e: u64| -> Result<_, DecodeStreamError<_, _, _>> {
                v.push(version.as_u64());
                Ok(v)
            },
        )
        .await
        .unwrap();

    assert_eq!(versions, vec![1, 2, 3]);
}

#[tokio::test]
async fn decoder_borrowing_no_upcaster_folds_payload_size() {
    let stream = VecStream::new(vec![
        (1, "Blob".into(), vec![1, 2, 3]),
        (2, "Blob".into(), vec![4, 5]),
        (3, "Blob".into(), vec![6, 7, 8, 9]),
    ]);
    let codec = BytesBorrowingCodec;

    let total = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            0usize,
            |sum, _v, bytes: &[u8]| -> Result<usize, DecodeStreamError<_, _, _>> {
                Ok(sum + bytes.len())
            },
        )
        .await
        .unwrap();

    assert_eq!(total, 9);
}

#[tokio::test]
async fn decoder_with_upcaster_applies_transform_before_codec() {
    // Upcaster prepends a 1, so 8-byte u64 payloads become 9 bytes — codec must fail.
    let stream = VecStream::new(vec![(1, "Tick".into(), 5u64.to_le_bytes().to_vec())]);
    let codec = U64Codec;
    let upcaster = PrependOne;

    let result = stream
        .decoder()
        .codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold(
            0u64,
            |sum, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(sum + e) },
        )
        .await;

    assert!(matches!(
        result,
        Err(DecodeStreamError::Codec(U64DecodeError(9)))
    ));
}

#[tokio::test]
async fn decoder_empty_stream_returns_initial() {
    let stream = VecStream::new(vec![]);
    let codec = U64Codec;

    let total = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            99u64,
            |sum, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(sum + e) },
        )
        .await
        .unwrap();

    assert_eq!(total, 99);
}

#[tokio::test]
async fn decoder_closure_error_short_circuits() {
    #[derive(Debug, thiserror::Error)]
    #[error("custom: {0}")]
    struct CustomErr(u64);

    impl<S, C, U> From<DecodeStreamError<S, C, U>> for CustomErr
    where
        S: std::fmt::Display,
        C: std::fmt::Display,
        U: std::fmt::Display,
    {
        fn from(_e: DecodeStreamError<S, C, U>) -> Self {
            CustomErr(0)
        }
    }

    let stream = VecStream::new(vec![
        (1, "E".into(), 10u64.to_le_bytes().to_vec()),
        (2, "E".into(), 99u64.to_le_bytes().to_vec()), // trigger error here
        (3, "E".into(), 30u64.to_le_bytes().to_vec()), // should not be folded
    ]);
    let codec = U64Codec;

    let result = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(0u64, |sum, _v, e: u64| -> Result<u64, CustomErr> {
            if e == 99 {
                Err(CustomErr(e))
            } else {
                Ok(sum + e)
            }
        })
        .await;

    assert!(matches!(result, Err(CustomErr(99))));
}
