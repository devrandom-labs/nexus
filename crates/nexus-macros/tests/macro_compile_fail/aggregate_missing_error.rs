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

#[nexus::aggregate(state = St, id = MyId)]
struct MissingError;

fn main() {}
