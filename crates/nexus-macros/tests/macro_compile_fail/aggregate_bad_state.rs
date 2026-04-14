/// state type must implement AggregateState.

use nexus::*;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MyId([u8; 8]);
impl MyId {
    fn new(id: u64) -> Self { Self(id.to_be_bytes()) }
}
impl std::fmt::Display for MyId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", u64::from_be_bytes(self.0)) }
}
impl AsRef<[u8]> for MyId {
    fn as_ref(&self) -> &[u8] { &self.0 }
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
