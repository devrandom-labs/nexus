#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use nexus::Version;
use nexus_store::Upcaster;
use nexus_store::upcasting::EventMorsel;

#[derive(Debug)]
struct TestError;
impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "test error")
    }
}
impl std::error::Error for TestError {}

#[allow(dead_code, reason = "referenced by macro attribute only")]
struct TestAggregate;

#[nexus_macros::transforms(aggregate = TestAggregate, error = TestError)]
impl TestTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn created_v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",v2");
        Ok(data)
    }

    #[transform(event = "OrderCreated", from = 2, to = 3)]
    fn created_v2_to_v3(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",v3");
        Ok(data)
    }

    #[transform(event = "OrderCancelled", from = 1, to = 2, rename = "OrderVoided")]
    fn cancelled_to_voided(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        Ok(payload.to_vec())
    }
}

#[test]
fn transforms_multi_step_upcast() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert_eq!(result.schema_version(), Version::new(3).unwrap());
    assert_eq!(result.payload(), b"data,v2,v3");
    assert_eq!(result.event_type(), "OrderCreated");
}

#[test]
fn transforms_rename() {
    let morsel = EventMorsel::borrowed("OrderCancelled", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert_eq!(result.event_type(), "OrderVoided");
    assert_eq!(result.schema_version(), Version::new(2).unwrap());
}

#[test]
fn transforms_passthrough_current_version() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::new(3).unwrap(), b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert!(result.is_borrowed(), "current version should pass through");
}

#[test]
fn transforms_unknown_event_passthrough() {
    let morsel = EventMorsel::borrowed("Unknown", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert!(result.is_borrowed());
}

#[test]
fn transforms_current_version_lookup() {
    assert_eq!(
        TestTransforms.current_version("OrderCreated"),
        Some(Version::new(3).unwrap())
    );
    assert_eq!(
        TestTransforms.current_version("OrderCancelled"),
        Some(Version::new(2).unwrap())
    );
    assert_eq!(TestTransforms.current_version("Unknown"), None);
}
