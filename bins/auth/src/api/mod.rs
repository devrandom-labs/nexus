use axum::{
    RequestPartsExt,
    extract::{FromRequestParts, Path},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use tracing::{debug, instrument};

#[derive(Debug)]
enum Version {
    V1,
}

impl<S> FromRequestParts<S> for Version
where
    S: Send + Sync,
{
    type Rejection = Response;

    #[instrument(skip(_state))]
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        debug!("checking routing based on version");
        let params: Path<HashMap<String, String>> =
            parts.extract().await.map_err(IntoResponse::into_response)?;

        let version = params
            .get("version")
            .ok_or_else(|| (StatusCode::NOT_FOUND, "version param missing").into_response())?;
        debug!("version: {}", version);

        match version.as_str() {
            "v1" => Ok(Version::V1),
            _ => Err((StatusCode::NOT_FOUND, "unknown version").into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Version;
    use axum::{
        Router, body::Body, extract::Request, http::StatusCode, response::Html, routing::get,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn handler(version: Version) -> Html<String> {
        Html(format!("received request with version {version:?}"))
    }

    fn app() -> Router {
        Router::new().route("/{version}/foo", get(handler))
    }

    #[tokio::test]
    async fn test_v1() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/v1/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(html, "received request with version V1");
    }

    #[tokio::test]
    async fn test_v2() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/v2/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(html, "unknown version");
    }
}
