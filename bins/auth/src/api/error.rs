use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::payload::Response as PawzResponse;
use thiserror::Error as TError;

use super::AppJson;

#[derive(Debug, TError)]
pub enum Error {
    #[error("Resource not found")]
    NotFound,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, response) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                PawzResponse::error("Resource not found", Some(StatusCode::NOT_FOUND.as_u16())),
            ),
        };

        (status, AppJson(response)).into_response()
    }
}
