/// id type must implement nexus::Id.

use nexus::*;

#[derive(Debug, Clone)]
enum Ev { A }
impl Message for Ev {}
impl DomainEvent for Ev { fn name(&self) -> &'static str { "A" } }

#[derive(Default, Debug)]
struct St;
impl AggregateState for St {
    type Event = Ev;
    fn apply(&mut self, _: &Ev) {}
    fn name(&self) -> &'static str { "S" }
}

#[derive(Debug, thiserror::Error)]
#[error("e")]
struct MyError;

// u64 does NOT implement nexus::Id
#[nexus::aggregate(state = St, error = MyError, id = u64)]
struct BadAggregate;

fn main() {}
