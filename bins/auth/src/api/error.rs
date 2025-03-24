#![allow(dead_code)]
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pawz::payload::Response as PawzResponse;
use serde::Serialize;
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
            Self::NotFound => get_response(
                StatusCode::NOT_FOUND,
                PawzResponse::error("Resource not found", Some(StatusCode::NOT_FOUND.as_u16())),
            ),
            Self::InternalServerError => get_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                PawzResponse::error(
                    "Internal server error",
                    Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
                ),
            ),
        }
    }
}

pub fn get_response<T>(status: StatusCode, response: PawzResponse<T>) -> Response
where
    T: Serialize + Debug,
{
    (status, response.into_response()).into_response()
}
