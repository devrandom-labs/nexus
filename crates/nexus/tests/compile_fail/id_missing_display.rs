/// Id trait requires Display. A type without Display cannot impl Id.

use nexus::kernel::Id;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BadId(u64);
// Missing: impl Display for BadId

impl Id for BadId {}

fn main() {}
