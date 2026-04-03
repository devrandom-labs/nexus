#![allow(clippy::unwrap_used, reason = "tests")]

use nexus_store::UpcastError;
use nexus_store::apply_upcasters;
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

// =============================================================================
// apply_upcasters() tests
// =============================================================================

#[test]
fn apply_upcasters_noop_when_no_match() {
    let upcasters: &[&dyn EventUpcaster] = &[&RenameUpcaster];
    let (t, v, p) = apply_upcasters(upcasters, "OrderPlaced", 1, b"data").unwrap();
    assert_eq!(t, "OrderPlaced");
    assert_eq!(v, 1);
    assert_eq!(p, b"data");
}

#[test]
fn apply_upcasters_chains_correctly() {
    struct V2ToV3;
    impl EventUpcaster for V2ToV3 {
        fn can_upcast(&self, event_type: &str, v: u32) -> bool {
            event_type == "UserRegistered" && v == 2
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("UserSignedUp".to_owned(), 3, p.to_vec())
        }
    }

    let v2_to_v3 = V2ToV3;
    let rename = RenameUpcaster;
    let upcasters: &[&dyn EventUpcaster] = &[&rename, &v2_to_v3];

    // V1 UserCreated → V2 UserRegistered → V3 UserSignedUp
    let (t, v, _) = apply_upcasters(upcasters, "UserCreated", 1, b"data").unwrap();
    assert_eq!(t, "UserSignedUp");
    assert_eq!(v, 3);
}

#[test]
fn apply_upcasters_rejects_version_downgrade() {
    struct DowngradeUpcaster;
    impl EventUpcaster for DowngradeUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".to_owned(), 0, p.to_vec())
        }
    }

    let upcasters: &[&dyn EventUpcaster] = &[&DowngradeUpcaster];
    let err = apply_upcasters(upcasters, "E", 1, b"data").unwrap_err();
    assert!(matches!(err, UpcastError::VersionNotAdvanced { .. }));
}

#[test]
fn apply_upcasters_rejects_same_version() {
    struct SameVersionUpcaster;
    impl EventUpcaster for SameVersionUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".to_owned(), 1, p.to_vec())
        }
    }

    let upcasters: &[&dyn EventUpcaster] = &[&SameVersionUpcaster];
    let err = apply_upcasters(upcasters, "E", 1, b"data").unwrap_err();
    assert!(matches!(err, UpcastError::VersionNotAdvanced { .. }));
}

#[test]
fn apply_upcasters_rejects_empty_event_type() {
    struct EmptyTypeUpcaster;
    impl EventUpcaster for EmptyTypeUpcaster {
        fn can_upcast(&self, _: &str, v: u32) -> bool {
            v == 1
        }
        fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            (String::new(), 2, p.to_vec())
        }
    }

    let upcasters: &[&dyn EventUpcaster] = &[&EmptyTypeUpcaster];
    let err = apply_upcasters(upcasters, "E", 1, b"data").unwrap_err();
    assert!(matches!(err, UpcastError::EmptyEventType { .. }));
}

#[test]
fn apply_upcasters_rejects_infinite_loop() {
    // This upcaster always advances by 1 and always says it can upcast,
    // so it will loop until the chain limit.
    struct AlwaysUpcaster;
    impl EventUpcaster for AlwaysUpcaster {
        fn can_upcast(&self, _: &str, _: u32) -> bool {
            true
        }
        fn upcast(&self, _: &str, v: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            ("E".to_owned(), v + 1, p.to_vec())
        }
    }

    let upcasters: &[&dyn EventUpcaster] = &[&AlwaysUpcaster];
    let err = apply_upcasters(upcasters, "E", 1, b"data").unwrap_err();
    assert!(
        matches!(err, UpcastError::ChainLimitExceeded { limit: 100, .. }),
        "expected ChainLimitExceeded, got {err:?}"
    );
}

#[test]
fn apply_upcasters_empty_slice_is_noop() {
    let upcasters: &[&dyn EventUpcaster] = &[];
    let (t, v, p) = apply_upcasters(upcasters, "E", 1, b"data").unwrap();
    assert_eq!(t, "E");
    assert_eq!(v, 1);
    assert_eq!(p, b"data");
}
