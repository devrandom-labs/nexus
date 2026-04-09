use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;

fn main() {
    let envelope = {
        let payload = vec![1, 2, 3];
        PersistedEnvelope::<()>::new_unchecked(Version::new(1).unwrap(), "E", 1, &payload, ())
    };
    let _ = envelope.payload();
}
