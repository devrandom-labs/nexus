use std::{
    any::{Any, TypeId},
    ops::Deref,
};
use thiserror::Error as Err;

//-------------------- Body--------------------//

#[derive(Debug, Err)]
pub enum Error {
    #[error("Could not get Body value")]
    CouldNotGetValue,
}

pub struct Body {
    inner: Box<dyn Any + Send + Sync>,
}

impl Body {
    pub fn new<T>(data: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Body {
            inner: Box::new(data),
        }
    }

    /// since the inner is trait object of Any
    /// it should have type_id to fetch
    pub fn type_id(&self) -> TypeId {
        (*self.inner).type_id()
    }

    pub fn get_as_ref<T>(&self) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        self.inner.downcast_ref::<T>()
    }

    pub fn get_as_mut<T>(&mut self) -> Option<&mut T>
    where
        T: Any + Send + Sync,
    {
        self.inner.downcast_mut::<T>()
    }

    pub fn get<T>(self) -> Result<Box<T>, Box<Error>>
    where
        T: Any + Send + Sync,
    {
        self.inner
            .downcast::<T>()
            .map_err(|_| Box::new(Error::CouldNotGetValue))
    }
}

impl Deref for Body {
    type Target = dyn Any;
    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

#[cfg(test)]
mod test {
    use super::Body;

    // TODO: test out diff types can be used to create a body
    // TODO: get concrete type from body
    // TODO: test body between threads and tokio
}
