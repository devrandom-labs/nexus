#![allow(dead_code)]
use crate::core::Error;
use crate::core::domain::Email;
use tracing::{debug, instrument};

#[derive(Debug)]
pub struct InitiateRegister {
    email: Email,
}

// can return domain errors, service interaction errors
// infra errors,
#[instrument]
pub async fn handler(command: InitiateRegister) -> Result<(), Error> {
    debug!(email = %command.email, "initiating registration");
    Ok(())
}
