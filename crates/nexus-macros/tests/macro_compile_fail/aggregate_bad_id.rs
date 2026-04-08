/// id type must implement nexus::Id.

use nexus::*;

#[derive(Debug, Clone)]
enum Ev { A }
impl Message for Ev {}
impl DomainEvent for Ev { fn name(&self) -> &'static str { "A" } }

#[derive(Default, Debug, Clone)]
struct St;
impl AggregateState for St {
    type Event = Ev;
    fn initial() -> Self { Self::default() }
    fn apply(self, _: &Ev) -> Self { self }
}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct MyError;

// u64 does NOT implement nexus::Id
#[nexus::aggregate(state = St, error = MyError, id = u64)]
struct BadAggregate;

fn main() {}
