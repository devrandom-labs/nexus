#[utoipa::path(post, path = "/login", tags = ["User Authentication"], operation_id = "loginUser", responses((status = OK, body = String, description = "Login User")))]
pub async fn route() -> &'static str {
    "login"
}
