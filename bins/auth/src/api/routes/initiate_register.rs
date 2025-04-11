#![allow(dead_code)]

use super::AppResult;
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use crate::api::AppJson;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct InitiateRegisterRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
}

pub async fn route(AppJson(_request): AppJson<InitiateRegisterRequest>) -> AppResult<StatusCode> {
    Ok(StatusCode::ACCEPTED)
}

// TODO: sends a command to aggregate to initiate_verification
