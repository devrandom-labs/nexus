use nexus::StreamId;
use nexus_store::pending_envelope;

fn main() {
    let _ = pending_envelope(StreamId::from_persisted("s1").unwrap()).event_type("E");
}
