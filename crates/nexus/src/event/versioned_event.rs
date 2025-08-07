#[derive(Debug)]
pub struct VersionedEvent<E> {
    pub version: u64,
    pub event: E,
}
