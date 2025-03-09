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

    #[test]
    fn diff_body_types() {
        let type_one = 10;
        let type_two = String::from("Hello");

        #[derive(Debug, PartialEq)]
        struct Person {
            name: String,
        }

        let type_three = Person {
            name: String::from("joel"),
        };

        let body_one = Body::new(type_one);
        let body_two = Body::new(type_two);
        let body_three = Body::new(type_three);

        assert!(body_one.get_as_ref::<i32>().is_some());
        assert_eq!(body_one.get_as_ref::<i32>().unwrap(), &10);

        assert!(body_two.get_as_ref::<String>().is_some());
        assert_eq!(
            body_two.get_as_ref::<String>().unwrap(),
            &String::from("Hello")
        );

        assert!(body_three.get_as_ref::<Person>().is_some());
        assert_eq!(
            body_three.get_as_ref::<Person>().unwrap(),
            &Person {
                name: String::from("joel")
            }
        );
    }

    // #[test]
    // fn body_type_clash() {}

    // TODO: test body between threads and tokio
}
