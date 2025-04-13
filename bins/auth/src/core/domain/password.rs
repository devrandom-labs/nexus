#![allow(dead_code)]
use super::Error;
use crate::commons::password_validation::validate;

pub struct Password(String);

impl Password {
    pub fn new(password: String) -> Result<Self, Error> {
        let password = validate(&password).map(|_| Password(password))?;
        Ok(password)
    }

    pub fn secret(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod test {
    use super::Password;

    #[test]
    fn valid_password_should_be_created() {
        let input = "ValidP@ss1".to_string();
        let password = Password::new(input.clone());
        assert!(password.is_ok());
        let password = password.unwrap();
        assert_eq!(&input, &password.secret());
    }
}
