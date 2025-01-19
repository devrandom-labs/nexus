use std::marker::PhantomData;
use thiserror::Error as Err;

#[derive(Debug, Err)]
pub enum Error {
    #[error("Cache key not found")]
    KeyNotFound,
}

pub struct Cache<C> {
    capacity: usize,
    _type: PhantomData<C>,
}

pub trait Cacher<C>
where
    C: Clone,
{
    fn get(&self) -> C;
    fn put(&mut self, data: C) -> Result<C, Error>;
}

impl<C> Cache<C> {
    pub fn new(capacity: usize) -> Self {
        Cache {
            capacity,
            _type: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
