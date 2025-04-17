#![allow(dead_code)]
use crate::core::Error;
use crate::core::domain::Email;
use nexus::Command;
use tracing::{debug, instrument};

#[derive(Debug)]
pub struct InitiateRegister {
    email: Email,
}

// can return domain errors, service interaction errors
// infra errors,
#[instrument]
pub async fn handler(command: Command<InitiateRegister>) -> Result<(), Error> {
    let payload = command.payload();
    debug!(email = %payload.email, command_id = %command.id(), "initiating registration");
    Ok(())
}
