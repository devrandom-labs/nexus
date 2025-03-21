#[utoipa::path(get, path = "/health", responses((status = OK, body = String, description = "Check Application Health")))]
pub async fn route() -> &'static str {
    "ok."
}
