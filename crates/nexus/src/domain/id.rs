use std::{fmt::Debug, hash::Hash};
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + 'static + ToString + AsRef<[u8]> {}
