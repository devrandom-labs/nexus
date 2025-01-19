#![allow(dead_code)]
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

pub trait Cacher {
    type Value;
    type Key;
    fn get(&self, key: Self::Key) -> Result<Self::Value, Error>;
    fn put(&mut self, key: Self::Key, data: Self::Value) -> Result<Self::Value, Error>;
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

    #[test]
    fn add_capacity_for_caching() {
        let cache = Cache::<i32>::new(10);
    }
}
