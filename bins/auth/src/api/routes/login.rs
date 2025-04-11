#![allow(dead_code)]

use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

#[derive(Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[utoipa::path(post, path = "/login", tags = ["User Authentication"], operation_id = "loginUser", responses((status = OK, body = String, description = "Login User")))]
pub async fn route() -> &'static str {
    "login"
}
