use std::{
    any::{Any, TypeId},
    ops::Deref,
};
use thiserror::Error as Err;

//-------------------- Body--------------------//
#[derive(Debug, Err, PartialEq)]
pub enum Error {
    #[error("Could not get Body value")]
    CouldNotGetValue,
    #[error("Type mismatch: expected {expected:?}, found {found:?}")]
    TypeMismatch { expected: TypeId, found: TypeId },
}

pub struct Body {
    inner: Box<dyn Any + Send + Sync>,
    type_id: TypeId,
}

impl Body {
    pub fn new<T>(data: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Body {
            inner: Box::new(data),
            type_id: TypeId::of::<T>(),
        }
    }

    /// since the inner is trait object of Any
    /// it should have type_id to fetch
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    pub fn get_as_ref<T>(&self) -> Result<&T, Error>
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();
        if self.type_id == type_id {
            self.inner
                .downcast_ref::<T>()
                .ok_or(Error::CouldNotGetValue)
        } else {
            Err(Error::TypeMismatch {
                expected: self.type_id,
                found: type_id,
            })
        }
    }

    pub fn get_as_mut<T>(&mut self) -> Result<&mut T, Error>
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();

        if self.type_id == type_id {
            self.inner
                .downcast_mut::<T>()
                .ok_or(Error::CouldNotGetValue)
        } else {
            Err(Error::TypeMismatch {
                expected: self.type_id,
                found: type_id,
            })
        }
    }

    pub fn get<T>(self) -> Result<Box<T>, Error>
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();
        if self.type_id == type_id {
            self.inner
                .downcast::<T>()
                .map_err(|_| Error::CouldNotGetValue)
        } else {
            Err(Error::TypeMismatch {
                expected: self.type_id,
                found: type_id,
            })
        }
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
    use super::{Body, Error};
    use std::any::TypeId;

    #[test]
    fn get_with_correct_type() {
        let input_type = 10;
        let mut input_body = Body::new(input_type);

        let actual_type = input_body.get_as_ref::<i32>();
        assert!(actual_type.is_ok());
        assert_eq!(actual_type.unwrap(), &10);

        let actual_type = input_body.get_as_mut::<i32>();
        assert!(actual_type.is_ok());
        assert_eq!(actual_type.unwrap(), &mut 10);

        let actual_type = input_body.get::<i32>();
        assert!(actual_type.is_ok());
        assert_eq!(*actual_type.unwrap(), 10);
    }

    #[test]
    fn get_with_incorrect_type() {
        let input_type = 10;
        let mut input_body = Body::new(input_type);
        let actual_type = input_body.get_as_ref::<u32>();
        assert!(actual_type.is_err());
        assert_eq!(
            actual_type.unwrap_err(),
            Error::TypeMismatch {
                expected: TypeId::of::<i32>(),
                found: TypeId::of::<u32>()
            }
        );

        let actual_type = input_body.get_as_mut::<u32>();
        assert!(actual_type.is_err());
        assert_eq!(
            actual_type.unwrap_err(),
            Error::TypeMismatch {
                expected: TypeId::of::<i32>(),
                found: TypeId::of::<u32>()
            }
        );

        let actual_type = input_body.get::<u32>();
        assert!(actual_type.is_err());
        assert_eq!(
            actual_type.unwrap_err(),
            Error::TypeMismatch {
                expected: TypeId::of::<i32>(),
                found: TypeId::of::<u32>()
            }
        );
    }

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

        assert!(body_one.get_as_ref::<i32>().is_ok());
        assert_eq!(body_one.get_as_ref::<i32>().unwrap(), &10);

        assert!(body_two.get_as_ref::<String>().is_ok());
        assert_eq!(
            body_two.get_as_ref::<String>().unwrap(),
            &String::from("Hello")
        );

        assert!(body_three.get_as_ref::<Person>().is_ok());
        assert_eq!(
            body_three.get_as_ref::<Person>().unwrap(),
            &Person {
                name: String::from("joel")
            }
        );
    }

    #[test]
    #[should_panic]
    fn body_type_clash() {
        let type_one = 10;

        let body_one = Body::new(type_one);
        assert!(body_one.get_as_ref::<String>().is_err());
        let _ = body_one.get_as_ref::<String>().unwrap();
    }

    #[tokio::test]
    async fn body_should_be_thread_safe() {
        let type_one = 10;
        let body_one = Body::new(type_one);
        let _ = tokio::spawn(async move {
            let inner_value = body_one.get_as_ref::<i32>();
            assert!(inner_value.is_ok());
            assert_eq!(inner_value.unwrap(), &10);
        })
        .await;
    }
}
