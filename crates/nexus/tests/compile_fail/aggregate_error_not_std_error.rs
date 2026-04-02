/// Aggregate::Error must implement std::error::Error.
/// A plain enum without #[derive(Error)] should fail.

use nexus::*;

#[derive(Debug, Clone)]
enum MyEvent { A }
impl Message for MyEvent {}
impl DomainEvent for MyEvent {
    fn name(&self) -> &'static str { "A" }
}

#[derive(Default, Debug)]
struct MyState;
impl AggregateState for MyState {
    type Event = MyEvent;
    fn initial() -> Self { Self::default() }
    fn apply(&mut self, _: &MyEvent) {}
    fn name(&self) -> &'static str { "My" }
}

// This error does NOT implement std::error::Error
#[derive(Debug)]
enum BadError {
    Something,
}

#[derive(Debug)]
struct MyAggregate;

impl Aggregate for MyAggregate {
    type State = MyState;
    type Error = BadError; // should fail: BadError doesn't impl Error
    type Id = MyId;
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MyId(u64);
impl std::fmt::Display for MyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl Id for MyId {}

fn main() {}
