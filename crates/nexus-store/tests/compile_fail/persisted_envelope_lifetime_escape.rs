use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;

fn main() {
    let envelope = {
        let stream_id = String::from("s1");
        let payload = vec![1, 2, 3];
        PersistedEnvelope::<()>::new(&stream_id, Version::from_persisted(1), "E", &payload, ())
    };
    let _ = envelope.stream_id();
}
