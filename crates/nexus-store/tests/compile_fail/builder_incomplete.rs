use nexus::Version;
use nexus_store::pending_envelope;

fn main() {
    let _ = pending_envelope(Version::new(1).unwrap())
        .event_type("E")
        .build_without_metadata();
}
