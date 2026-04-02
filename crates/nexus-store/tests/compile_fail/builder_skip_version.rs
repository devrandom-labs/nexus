use nexus_store::pending_envelope;

fn main() {
    let _ = pending_envelope("s1".into()).event_type("E");
}
