//! Tests for the compile-time upcaster chain.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus_store::upcaster::EventUpcaster;
use nexus_store::upcaster_chain::{Chain, UpcasterChain};

struct V1ToV2;
impl EventUpcaster for V1ToV2 {
    fn can_upcast(&self, event_type: &str, v: u32) -> bool {
        event_type == "UserCreated" && v == 1
    }
    fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
        ("UserCreated".to_owned(), 2, p.to_vec())
    }
}

struct V2ToV3;
impl EventUpcaster for V2ToV3 {
    fn can_upcast(&self, event_type: &str, v: u32) -> bool {
        event_type == "UserCreated" && v == 2
    }
    fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
        ("UserRegistered".to_owned(), 3, p.to_vec())
    }
}

#[test]
fn empty_chain_returns_none() {
    let chain: () = ();
    assert!(chain.try_upcast("UserCreated", 1, b"data").is_none());
}

#[test]
fn single_upcaster_matches() {
    let chain = Chain(V1ToV2, ());
    let result = chain.try_upcast("UserCreated", 1, b"data");
    assert!(result.is_some());
    let (t, v, _) = result.unwrap();
    assert_eq!(t, "UserCreated");
    assert_eq!(v, 2);
}

#[test]
fn single_upcaster_no_match() {
    let chain = Chain(V1ToV2, ());
    assert!(chain.try_upcast("OrderPlaced", 1, b"data").is_none());
}

#[test]
fn chain_dispatches_to_correct_upcaster() {
    let chain = Chain(V2ToV3, Chain(V1ToV2, ()));

    let (t1, v1, _) = chain.try_upcast("UserCreated", 1, b"data").unwrap();
    assert_eq!(t1, "UserCreated");
    assert_eq!(v1, 2);

    let (t2, v2, _) = chain.try_upcast("UserCreated", 2, b"data").unwrap();
    assert_eq!(t2, "UserRegistered");
    assert_eq!(v2, 3);
}

#[test]
fn chain_no_match_returns_none() {
    let chain = Chain(V2ToV3, Chain(V1ToV2, ()));
    assert!(chain.try_upcast("UserCreated", 3, b"data").is_none());
}

#[test]
fn chain_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<()>();
    assert_send_sync::<Chain<V1ToV2, ()>>();
    assert_send_sync::<Chain<V2ToV3, Chain<V1ToV2, ()>>>();
}

#[test]
fn chain_is_zero_sized_when_upcasters_are_stateless() {
    assert_eq!(std::mem::size_of::<()>(), 0);
    assert_eq!(std::mem::size_of::<Chain<V1ToV2, ()>>(), 0);
    assert_eq!(std::mem::size_of::<Chain<V2ToV3, Chain<V1ToV2, ()>>>(), 0);
}
