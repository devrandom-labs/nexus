#![allow(dead_code)]
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::payload::{ApiReply, Reply};
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
        match self {
            Self::NotFound => ApiReply::new(
                StatusCode::NOT_FOUND,
                Reply::error("Resource not found", Some(StatusCode::NOT_FOUND.as_u16())),
            )
            .into_response(),
            Self::InternalServerError => ApiReply::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                Reply::error(
                    "Internal server error",
                    Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
                ),
            )
            .into_response(),
        }
    }
}
