/// aggregate macro must require `error`.

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

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct MyId(u64);
impl std::fmt::Display for MyId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl Id for MyId {}

#[nexus::aggregate(state = St, id = MyId)]
struct MissingError;

fn main() {}
