use std::fmt::{Debug, Display};
use std::hash::Hash;

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {
    /// Fixed byte length of this ID's storage representation.
    /// All instances must return exactly this many bytes from `as_ref()`.
    const BYTE_LEN: usize;
}
