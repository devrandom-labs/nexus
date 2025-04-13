#![allow(dead_code)]
use super::error::Error;
use argon2::{Algorithm, Argon2, Params, Version};
use tracing::{error, instrument};

use password_hash::{
    PasswordHashString, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};

#[derive(Debug)]
pub struct PasswordSecurity<'a> {
    hasher: Argon2<'a>,
}

impl PasswordSecurity<'_> {
    pub fn new() -> Self {
        Self {
            hasher: Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default()),
        }
    }

    #[instrument]
    pub fn hash_password(&self, password: &str) -> Result<PasswordHashString, Error> {
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = self
            .hasher
            .hash_password(password.as_bytes(), &salt)
            .inspect_err(|err| error!(?err))
            .map_err(Error::PasswordHash)?
            .serialize();
        Ok(password_hash)
    }

    #[instrument]
    pub fn verify_password(
        &self,
        password: &str,
        password_hash: &PasswordHashString,
    ) -> Result<(), Error> {
        let password_hash = password_hash.password_hash();
        self.hasher
            .verify_password(password.as_bytes(), &password_hash)
            .inspect_err(|err| error!(?err))
            .map_err(Error::PasswordHash)
    }
}

// TODO: create a trait to impl securities with argon2
// TODO: prettify the error logs here.
