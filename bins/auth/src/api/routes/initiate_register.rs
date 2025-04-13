#![allow(dead_code)]
use super::AppResult;
use crate::api::AppJson;
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::{debug, instrument};
use utoipa::ToSchema;
use validator::Validate;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct InitiateRegisterRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
}

#[utoipa::path(post,
               path = "/initiate_register",
               tags = ["Onboarding"],
               operation_id = "initiateRegister",
               request_body = InitiateRegisterRequest,
               responses(
                   (status = ACCEPTED, description = "Verification link/otp has been sent to the email address", content_type = "application/json"),
               )
)]
#[instrument(name = "initiate_register", target = "api::auth::initiate_register")]
pub async fn route(AppJson(request): AppJson<InitiateRegisterRequest>) -> AppResult<StatusCode> {
    debug!(?request);
    Ok(StatusCode::ACCEPTED)
}

// TODO: sends a command to aggregate to initiate_verification

#[cfg(test)]
mod test {

    // send valid body -> should return status::accepted
    // send invalid body -> should return someting (lets find out)
}
