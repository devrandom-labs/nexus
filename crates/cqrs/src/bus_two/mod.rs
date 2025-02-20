use thiserror::Error as Err;

#[derive(Err, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Error {}

// Trait Object, TypeId, 'static lifetime messages and runtime type reflection.
use std::any::Any;

pub trait Message: Any + Send + Sync + 'static {}

// blanket impl, I think this is okay for now.
impl<T: Any + Send + Sync + 'static> Message for T {}
