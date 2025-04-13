#![allow(dead_code)]
use super::Error;
use crate::commons::password_validation::validate;

pub struct Password(String);

impl Password {
    pub fn new(password: String) -> Result<Self, Error> {
        let password = validate(&password).map(|_| Password(password))?;
        Ok(password)
    }
}
