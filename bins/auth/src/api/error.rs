#![allow(dead_code)]
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::payload::{Reply, ReplyInner};
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
            Self::NotFound => Reply::new(
                StatusCode::NOT_FOUND,
                ReplyInner::error("Resource not found", Some(StatusCode::NOT_FOUND.as_u16())),
            )
            .into_response(),
            Self::InternalServerError => Reply::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ReplyInner::error(
                    "Internal server error",
                    Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
                ),
            )
            .into_response(),
        }
    }
}
