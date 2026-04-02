use nexus::*;
use nexus_store::codec::Codec;

// A simple test codec that encodes via Debug format
#[derive(Clone)]
struct DebugCodec;

#[derive(Debug, Clone, PartialEq)]
enum TestEvent {
    Created(String),
    Deleted,
}
impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "Created",
            Self::Deleted => "Deleted",
        }
    }
}

impl Codec for DebugCodec {
    type Error = std::io::Error;

    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error> {
        Ok(format!("{event:?}").into_bytes())
    }

    fn decode<E: DomainEvent>(&self, _event_type: &str, _payload: &[u8]) -> Result<E, Self::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "DebugCodec can only encode",
        ))
    }
}

#[test]
fn codec_encode_produces_bytes() {
    let codec = DebugCodec;
    let event = TestEvent::Created("hello".into());
    let bytes = codec.encode(&event).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn codec_encode_unit_variant() {
    let codec = DebugCodec;
    let event = TestEvent::Deleted;
    let bytes = codec.encode(&event).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(event.name(), "Deleted");
}

#[test]
fn codec_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DebugCodec>();
}
