/// state type must implement AggregateState.

use nexus::*;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MyId(u64);
impl std::fmt::Display for MyId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl Id for MyId {}

// NotAState does NOT implement AggregateState
#[derive(Default, Debug)]
struct NotAState;

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct MyError;

#[nexus::aggregate(state = NotAState, error = MyError, id = MyId)]
struct BadAggregate;

fn main() {}
