#![allow(dead_code)]
use axum::{
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::jsend::Body;
use std::fmt::Debug;
use thiserror::Error as TError;
use validator::ValidationErrors;

#[derive(Debug, TError)]
pub enum Error {
    #[error("Resource not found")]
    NotFound,
    #[error("Internal server error")]
    #[allow(clippy::enum_variant_names)]
    InternalServerError,
    #[error("Invalid Json Request: {0}")]
    JsonRejection(#[from] JsonRejection),
    #[error("Validation Error: {0}")]
    Validation(#[from] ValidationErrors),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Resource not found".to_owned()),
            Self::InternalServerError => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_owned(),
            ),
            Self::JsonRejection(rejection) => (rejection.status(), rejection.body_text()),
            Self::Validation(err) => (StatusCode::BAD_REQUEST, err.to_string()),
        };
        (
            status,
            Body::<()>::error(message, Some(status.as_u16()), None),
        )
            .into_response()
    }
}
