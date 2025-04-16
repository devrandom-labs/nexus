#![allow(dead_code)]
use super::AppResult;
use axum::http::StatusCode;
use pawz::jsend::Body;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use crate::api::ValidJson;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct VerifyEmailRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
    otp: String,
}

// TODO: figure out the flow for this, verify first methodolgy

#[derive(Serialize, ToSchema)]
pub struct VerifyEmailResponse {
    token: String,
}

#[utoipa::path(post,
               path = "/verify_email",
               tags = ["Onboarding"],
               operation_id = "verifyEmail",
               request_body = VerifyEmailRequest,
               responses(
                   (status = OK, body = Body<VerifyEmailResponse> ,description = "Email has been successfully verified", content_type = "application/json")
               )
)]
#[instrument(name = "verify_email", target = "api::auth::verify_email")]
pub async fn route(
    ValidJson(_request): ValidJson<VerifyEmailRequest>,
) -> AppResult<(StatusCode, ValidJson<Body<VerifyEmailResponse>>)> {
    Ok((
        StatusCode::OK,
        ValidJson(Body::success(VerifyEmailResponse {
            token: "Some token".to_string(),
        })),
    ))
}
