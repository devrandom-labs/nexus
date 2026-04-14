use std::fmt::{Debug, Display};
use std::hash::Hash;

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {}
