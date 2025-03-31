use super::{body::Body, error::Error as PayloadError};
use axum::http::StatusCode;
use serde::Serialize;

type ReplyResult<T> = Result<T, PayloadError>;

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

impl<T> Error<T>
where
    T: Serialize,
{
    fn set_message(&mut self, message: String) -> Result<(), PayloadError> {
        if message.trim().is_empty() {
            return Err(PayloadError::EmptyMessage);
        }
        self.message = message;
        Ok(())
    }

    fn transform<B, F>(self, transform: F) -> Error<B>
    where
        B: Serialize,
        F: FnOnce(T) -> B,
    {
        Error {
            body: self.body.map(transform),
            code: self.code,
            message: self.message,
        }
    }
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
    r#type: Option<T>,
}

pub fn error<T>() -> Reply<Error<T>>
where
    T: Serialize,
{
    let error = Error::default();
    Reply {
        r#type: Some(error),
    }
}

pub fn success<T>(body: T) -> Reply<Success<T>>
where
    T: Serialize,
{
    let success = Success { body };
    Reply {
        r#type: Some(success),
    }
}

pub fn fail<T>(body: T) -> Reply<Fail<T>>
where
    T: Serialize,
{
    let fail = Fail { body };
    Reply { r#type: Some(fail) }
}

impl Default for Reply<Success<String>> {
    fn default() -> Self {
        Self {
            r#type: Some(Success::default()),
        }
    }
}

impl Default for Reply<Error<String>> {
    fn default() -> Self {
        Self {
            r#type: Some(Error::default()),
        }
    }
}

impl Default for Reply<Fail<String>> {
    fn default() -> Self {
        Self {
            r#type: Some(Fail::default()),
        }
    }
}

impl<T> Reply<Error<T>>
where
    T: Serialize,
{
    pub fn with_body(mut self, body: T) -> ReplyResult<Reply<Error<T>>> {
        self.r#type
            .as_mut()
            .map(|r_type| r_type.body = Some(body))
            .ok_or(PayloadError::NoTypeSchema)?;
        Ok(self)
    }

    pub fn with_message(mut self, message: String) -> ReplyResult<Reply<Error<T>>> {
        self.r#type
            .as_mut()
            .ok_or(PayloadError::NoTypeSchema)
            .and_then(|r| r.set_message(message))?;
        Ok(self)
    }

    pub fn with_code(mut self, code: u16) -> ReplyResult<Reply<Error<T>>> {
        self.r#type
            .as_mut()
            .map(|r| r.code = Some(code))
            .ok_or(PayloadError::NoTypeSchema)?;
        Ok(self)
    }

    pub fn build(self) -> ReplyResult<Body<T>> {
        let Error {
            body,
            message,
            code,
        } = self.r#type.ok_or(PayloadError::NoTypeSchema)?;
        Ok(Body::error(message, code, body)?)
    }
}

impl<T> Reply<Fail<T>>
where
    T: Serialize,
{
    pub fn build(self) -> ReplyResult<Body<T>> {
        self.r#type
            .ok_or(PayloadError::NoTypeSchema)
            .map(|b| Body::fail(b.body))
    }
}

impl<T> Reply<Success<T>>
where
    T: Serialize,
{
    pub fn build(self) -> ReplyResult<Body<T>> {
        self.r#type
            .ok_or(PayloadError::NoTypeSchema)
            .map(|b| Body::success(b.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize, PartialEq, Debug)]
    struct TestData {
        value: String,
    }

    #[test]
    fn test_success_reply() {
        let reply = success(TestData {
            value: "test".to_string(),
        });
        let body = reply.build().unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"status": "success", "data": {"value": "test"}})
        );
    }

    #[test]
    fn test_fail_reply() {
        let reply = fail(TestData {
            value: "error".to_string(),
        });
        let body = reply.build().unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"status": "fail", "data": {"value": "error"}})
        );
    }

    #[test]
    fn test_error_reply() {
        let reply: Reply<Error<TestData>> = error();
        let body = reply
            .with_message("test error".to_string())
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"status": "error", "message": "test error", "code": 500})
        );
    }

    #[test]
    fn test_error_reply_with_body() {
        let reply: Reply<Error<TestData>> = error();
        let body = reply
            .with_body(TestData {
                value: "test body".to_string(),
            })
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"status": "error", "message": "internal server error", "code": 500, "data": {"value": "test body"}})
        );
    }

    #[test]
    fn test_error_reply_with_code() {
        let reply: Reply<Error<TestData>> = error();
        let body = reply.with_code(400).unwrap().build().unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"status": "error", "message": "internal server error", "code": 400})
        );
    }

    #[test]
    fn test_error_set_message_empty() {
        let mut error = Error::<TestData>::default();
        assert_eq!(
            error.set_message(" ".to_string()).unwrap_err(),
            PayloadError::EmptyMessage
        );
    }

    #[test]
    fn test_error_transform() {
        let error_a = Error {
            body: Some(TestData {
                value: "test".to_string(),
            }),
            message: "test message".to_string(),
            code: Some(400),
        };
        let error_b: Error<String> = error_a.transform(|data| data.value);
        assert_eq!(error_b.body, Some("test".to_string()));
    }

    #[test]
    fn test_default_success_reply() {
        let reply: Reply<Success<String>> = Reply::default();
        assert_eq!(reply.r#type.unwrap().body, "ok.".to_string());
    }

    #[test]
    fn test_default_fail_reply() {
        let reply: Reply<Fail<String>> = Reply::default();
        assert_eq!(reply.r#type.unwrap().body, "not ok".to_string());
    }

    #[test]
    fn test_default_error_reply() {
        let reply: Reply<Error<String>> = Reply::default();
        let r_type = reply.r#type.unwrap();
        assert_eq!(r_type.message, "internal server error".to_string());
        assert_eq!(r_type.code, Some(500));
    }
}
