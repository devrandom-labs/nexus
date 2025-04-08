// TODO: takes only an email address
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct InitiateRegisterRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
}
