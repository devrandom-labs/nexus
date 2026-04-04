use nexus::{StreamId, Version};
use nexus_store::pending_envelope;

fn main() {
    let _ = pending_envelope(StreamId::from_persisted("s1").unwrap())
        .version(Version::from_persisted(1))
        .event_type("E")
        .build_without_metadata();
}
