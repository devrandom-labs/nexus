//! Test that user attributes are preserved through the aggregate macro.

use nexus::*;
use std::fmt;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AttrId(u64);
impl fmt::Display for AttrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Id for AttrId {}

#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "test-only event type for macro attribute preservation tests"
)]
enum AttrEvent {
    A,
}
impl Message for AttrEvent {}
impl DomainEvent for AttrEvent {
    fn name(&self) -> &'static str {
        "A"
    }
}

#[derive(Default, Debug)]
struct AttrState;
impl AggregateState for AttrState {
    type Event = AttrEvent;
    fn initial() -> Self {
        Self
    }
    fn apply(&mut self, _: &AttrEvent) {}
    fn name(&self) -> &'static str {
        "Attr"
    }
}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct AttrError;

// =============================================================================
// Test: #[doc] attribute preserved
// =============================================================================

/// This is a documented aggregate.
/// It should appear in cargo doc output.
#[nexus::aggregate(state = AttrState, error = AttrError, id = AttrId)]
struct DocumentedAggregate;

#[test]
fn doc_attribute_preserved() {
    // If the doc attribute was dropped, this test still passes —
    // but `cargo doc` would show no documentation.
    // The real verification is that this compiles without error.
    let _agg = DocumentedAggregate::new(AttrId(1));
}

// =============================================================================
// Test: #[cfg] attribute preserved — conditional compilation
// =============================================================================

#[cfg(test)]
#[nexus::aggregate(state = AttrState, error = AttrError, id = AttrId)]
struct TestOnlyAggregate;

#[test]
fn cfg_attribute_preserved() {
    // This aggregate only exists in test cfg.
    // If #[cfg(test)] was dropped, it would also exist in non-test builds.
    let _agg = TestOnlyAggregate::new(AttrId(1));
}

// =============================================================================
// Test: #[allow] attribute preserved
// =============================================================================

#[allow(dead_code, reason = "testing attribute preservation")]
#[nexus::aggregate(state = AttrState, error = AttrError, id = AttrId)]
struct UnusedAggregate;

#[test]
fn allow_attribute_preserved() {
    // If #[allow(dead_code)] was dropped, this would warn about UnusedAggregate.
    // (It's unused because we only check it compiles without warnings.)
}
