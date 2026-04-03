use nexus::Version;
use nexus_store::pending_envelope;

fn main() {
    let _ = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .build_without_metadata();
}
