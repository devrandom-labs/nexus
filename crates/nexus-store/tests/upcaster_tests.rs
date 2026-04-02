use nexus_store::upcaster::EventUpcaster;

struct RenameUpcaster;

impl EventUpcaster for RenameUpcaster {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        event_type == "UserCreated" && schema_version < 2
    }

    fn upcast(
        &self,
        _event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        ("UserRegistered".to_owned(), 2, payload.to_vec())
    }
}

#[test]
fn upcaster_identifies_upgradeable_events() {
    let upcaster = RenameUpcaster;
    assert!(upcaster.can_upcast("UserCreated", 1));
    assert!(!upcaster.can_upcast("UserCreated", 2));
    assert!(!upcaster.can_upcast("OrderPlaced", 1));
}

#[test]
fn upcaster_transforms_event() {
    let upcaster = RenameUpcaster;
    let payload = b"original payload";
    let (new_type, new_version, new_payload) = upcaster.upcast("UserCreated", 1, payload);

    assert_eq!(new_type, "UserRegistered");
    assert_eq!(new_version, 2);
    assert_eq!(new_payload, payload);
}

#[test]
fn upcaster_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RenameUpcaster>();
}
