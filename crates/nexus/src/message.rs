use std::fmt::Debug;

pub trait Message: Send + Sync + Debug + 'static {}
