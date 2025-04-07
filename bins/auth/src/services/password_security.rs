use super::error::Error;
use argon2::Argon2;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng};

pub struct PasswordSecurity;

impl PasswordSecurity {
    pub fn hash_password(password: &str) -> Result<PasswordHash, Error> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2.hash_password(password.as_bytes(), &salt)?;
        Ok(password_hash)
    }
    pub fn verify_password(password: &str, password_hash: &PasswordHash) -> Result<(), Error> {
        Argon2::default()
            .verify_password(password.as_bytes(), password_hash)
            .map_err(|err| Error::PasswordHash(err))
    }
}

// TODO: change to password security
// TODO: create a trait to impl securities with argon2
