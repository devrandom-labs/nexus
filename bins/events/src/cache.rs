use std::marker::PhantomData;
use thiserror::Error as Err;

#[derive(Debug, Err)]
pub enum Error {
    #[error("Cache key not found")]
    KeyNotFound,
}

pub struct Cache<C> {
    _type: PhantomData<C>,
}

pub trait Cacher<C>
where
    C: Clone,
{
    fn get(&self) -> C;
    fn put(&mut self, data: C) -> Result<C, Error>;
}
