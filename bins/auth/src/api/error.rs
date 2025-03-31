#![allow(dead_code)]
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::Body as PawzBody;
use std::fmt::Debug;
use thiserror::Error as TError;

#[derive(Debug, TError)]
pub enum Error {
    #[error("Resource not found")]
    NotFound,
    #[error("Internal server error")]
    InternalServerError,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let builder = Response::builder();
        match self {
            Self::NotFound => builder
                .status(StatusCode::NOT_FOUND)
                .body(PawzBody::error_no_body("Resource not found", None))
                .unwrap(),
            Self::InternalServerError => builder
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(PawzBody::error_no_body("Internal server error", None))
                .unwrap(),
        }
    }
}
