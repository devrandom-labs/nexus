use std::{fmt::Debug, hash::Hash};

pub trait ReadModel: Send + Sync + Debug + 'static {
    type Id: Send + Sync + Debug + Clone + Eq + Hash + 'static;
}
