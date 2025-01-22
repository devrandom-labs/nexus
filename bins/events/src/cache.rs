#![allow(dead_code)]
use std::collections::HashMap;
use thiserror::Error as Err;

#[derive(Debug, Err)]
pub enum Error {
    #[error("Cache key not found")]
    KeyNotFound,
}

pub struct Cache<K, V> {
    capacity: usize,
    store: HashMap<K, V>,
}

pub trait Cacher {
    type Value;
    type Key;
    fn get(&self, key: Self::Key) -> Result<Self::Value, Error>;
    fn put(&mut self, key: Self::Key, data: Self::Value) -> Result<Self::Value, Error>;
}

impl<K, V> Cache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Cache {
            capacity,
            store: HashMap::new(),
        }
    }
}
