/// Version cannot be constructed via From<u64>.
/// Only Version::INITIAL, Version::from_persisted(), and Version::next() are available.

use nexus::kernel::Version;

fn main() {
    let _v: Version = Version::from(42u64);
}
