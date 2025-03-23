use axum::response::IntoResponse;

#[utoipa::path(post, path = "/register", tags = ["User Authentication"], operation_id = "registerUser", responses((status = OK, body = String, description = "Register User")))]
pub async fn route() -> impl IntoResponse {
    "register"
}
