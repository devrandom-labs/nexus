use super::body::Body;
use axum::http::StatusCode;
use serde::Serialize;

pub trait ReplyType: Serialize {
    fn default_status() -> StatusCode;
}

#[derive(Serialize)]
pub struct Success<T>
where
    T: Serialize,
{
    body: T,
}

impl Default for Success<String> {
    fn default() -> Self {
        Self {
            body: "ok.".to_string(),
        }
    }
}

impl<T> ReplyType for Success<T>
where
    T: Serialize,
{
    fn default_status() -> StatusCode {
        StatusCode::ACCEPTED
    }
}

#[derive(Serialize)]
pub struct Fail<T>
where
    T: Serialize,
{
    body: T,
}

impl Default for Fail<String> {
    fn default() -> Self {
        Self {
            body: "not ok".to_string(),
        }
    }
}

impl<T> ReplyType for Fail<T>
where
    T: Serialize,
{
    fn default_status() -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

#[derive(Serialize)]
pub struct Error<T>
where
    T: Serialize,
{
    body: Option<T>,
    message: String,
    code: Option<u16>,
}

impl<T> Default for Error<T>
where
    T: Serialize,
{
    fn default() -> Self {
        Self {
            body: None,
            message: "internal server error".to_string(),
            code: Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
        }
    }
}

impl<T> ReplyType for Error<T>
where
    T: Serialize,
{
    fn default_status() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

pub struct Reply<T>
where
    T: ReplyType,
{
    r#type: T,
}

impl Default for Reply<Success<String>> {
    fn default() -> Self {
        Self {
            r#type: Success::default(),
        }
    }
}

impl Default for Reply<Error<String>> {
    fn default() -> Self {
        Self {
            r#type: Error::default(),
        }
    }
}

impl Default for Reply<Fail<String>> {
    fn default() -> Self {
        Self {
            r#type: Fail::default(),
        }
    }
}

impl<T> Reply<T>
where
    T: ReplyType,
{
    pub fn error() -> Reply<Error<String>> {
        Reply::<Error<String>>::default()
    }

    pub fn success() -> Reply<Success<String>> {
        Reply::<Success<String>>::default()
    }

    pub fn fail() -> Reply<Fail<String>> {
        Reply::<Fail<String>>::default()
    }
}

impl<T> Reply<Error<T>>
where
    T: Serialize,
{
    pub fn with_body(mut self, body: T) -> Reply<Error<T>> {
        self.r#type.body = Some(body);
        self
    }

    pub fn with_message(mut self, message: String) -> Reply<Error<T>> {
        self.r#type.message = message;
        self
    }

    pub fn with_code(mut self, code: u16) -> Reply<Error<T>> {
        self.r#type.code = Some(code);
        self
    }

    pub fn build(self) -> Body<T> {
        let Error {
            body,
            message,
            code,
        } = self.r#type;
        Body::error_with_body(message, code, body)
    }
}

impl<T> Reply<Fail<T>>
where
    T: Serialize,
{
    pub fn with_body(mut self, body: T) -> Reply<Fail<T>> {
        self.r#type.body = body;
        self
    }

    pub fn build(self) -> Body<T> {
        Body::fail(self.r#type.body)
    }
}

impl<T> Reply<Success<T>>
where
    T: Serialize,
{
    pub fn with_body(mut self, body: T) -> Reply<Success<T>> {
        self.r#type.body = body;
        self
    }

    pub fn build(self) -> Body<T> {
        Body::success(self.r#type.body)
    }
}
