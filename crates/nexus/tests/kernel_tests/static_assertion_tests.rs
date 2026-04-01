//! Static assertions for the kernel.
//!
//! These are compile-time checks — they produce zero runtime code.
//! If any assertion fails, the crate won't compile.
//! If they all pass, it's as if this file doesn't exist at runtime.

use nexus::kernel::*;
use nexus::kernel::aggregate::AggregateRoot;
use nexus::kernel::events::Events;
use nexus::kernel::version::VersionedEvent;
use static_assertions::*;
use std::fmt;

// =============================================================================
// Version: must be a zero-cost wrapper around u64
// =============================================================================

// Version is exactly the size of u64 — proof it's zero-cost
assert_eq_size!(Version, u64);

// Version has value semantics: Copy, Clone, Eq, Ord, Hash
assert_impl_all!(Version: Copy, Clone, Eq, Ord, std::hash::Hash);

// Version is thread-safe
assert_impl_all!(Version: Send, Sync);

// Version is displayable and debuggable
assert_impl_all!(Version: fmt::Display, fmt::Debug);

// =============================================================================
// KernelError: must be a proper std::error::Error
// =============================================================================

assert_impl_all!(KernelError: std::error::Error, Send, Sync, fmt::Debug, fmt::Display);

// =============================================================================
// VersionedEvent: generic checks
// =============================================================================

// VersionedEvent<T> is Send+Sync when T is
assert_impl_all!(VersionedEvent<u8>: Send, Sync);
assert_impl_all!(VersionedEvent<String>: Send, Sync);

// =============================================================================
// To test AggregateRoot, we need a concrete Aggregate.
// Define a minimal one here.
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct StaticId(u64);
impl fmt::Display for StaticId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for StaticId {}

#[derive(Debug, Clone)]
enum StaticEvent {
    A,
}
impl Message for StaticEvent {}
impl DomainEvent for StaticEvent {
    fn name(&self) -> &'static str {
        "A"
    }
}

#[derive(Default, Debug)]
struct StaticState;
impl AggregateState for StaticState {
    type Event = StaticEvent;
    fn apply(&mut self, _event: &StaticEvent) {}
    fn name(&self) -> &'static str {
        "Static"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("static error")]
struct StaticError;

#[derive(Debug)]
struct StaticAggregate;
impl Aggregate for StaticAggregate {
    type State = StaticState;
    type Error = StaticError;
    type Id = StaticId;
}

// =============================================================================
// AggregateRoot: must be Send + Sync for async usage in outer layers
// =============================================================================

assert_impl_all!(AggregateRoot<StaticAggregate>: Send, Sync, fmt::Debug);

// =============================================================================
// Events collection: must be Send + Sync when event type is
// =============================================================================

assert_impl_all!(Events<StaticEvent>: Send, Sync, fmt::Debug);

// =============================================================================
// Dummy test so cargo test recognizes this file
// =============================================================================

#[test]
fn static_assertions_compile() {
    // If this file compiles, all assertions passed.
    // This test exists only so `cargo test` reports it.
}
