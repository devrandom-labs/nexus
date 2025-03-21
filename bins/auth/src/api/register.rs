#[utoipa::path(post, path = "/register", responses((status = OK, body = String, description = "Register User")))]
pub async fn route() -> &'static str {
    "register"
}
