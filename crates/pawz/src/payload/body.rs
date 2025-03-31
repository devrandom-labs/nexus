use super::error::Error;
use serde::Serialize;

/// Represents a generic API response, based on the JSend specification.
///
/// This enum provides a structured way to represent different types of API responses,
/// including success, failure, and error scenarios, adhering to the JSend format:
/// [https://github.com/omniti-labs/jsend](https://github.com/omniti-labs/jsend).
///
/// **JSend Rationale:** JSend specifies that error messages should be strings
/// and may have an optional numeric code. This is reflected in the `Error` variant.
///
/// # Type Parameters
///
/// * `T`: The type of data associated with success and failure responses. This type
///         must implement `Debug` and `Serialize`.
///
/// # Variants
///
/// * `Success { data: T }`: Represents a successful response containing data of type `T`.
/// * `Fail { data: T }`: Represents a failed response containing data of type `T`.
/// * `Error { message: String, code: Option<u16>, data: Option<T> }`: Represents an error
///         response with a message, an optional error code, and optional data.
///
/// # Serialization
///
/// The `status` field in the serialized JSON will be "success", "fail", or "error"
/// based on the variant. The `code` field in the `Error` variant is omitted if `None`.
///
/// # Examples
///
/// ```rust
/// use serde::Serialize;
/// use pawz::payload::body::Body;
///
/// #[derive(Serialize)]
/// struct User {
///     id: u32,
///     name: String,
/// }
///
/// let success_response: Body<User> = Body::success(User {
///     id: 123,
///     name: "Alice".to_string(),
/// });
///
/// let error_response: Body<String> = Body::error("User not found", Some(404), None).unwrap();
/// ```
#[derive(Serialize)]
#[serde(tag = "status")]
pub enum Body<T>
where
    T: Serialize,
{
    #[serde(rename = "success")]
    Success { data: T },
    #[serde(rename = "fail")]
    Fail { data: T },
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<T>,
    },
}

impl<T> Body<T>
where
    T: Serialize,
{
    /// Constructs a `Success` response with the provided data.
    ///
    /// This constructor is generic and can be used with any type `T` that implements
    /// `Debug` and `Serialize`.
    ///
    /// # Arguments
    ///
    /// * `data`: The data to include in the success response.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use serde::Serialize;
    /// use pawz::payload::body::Body;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// let success_response: Body<User> = Body::success(User {
    ///     id: 123,
    ///     name: "Alice".to_string(),
    /// });
    /// ```
    pub fn success(data: T) -> Self {
        Body::Success { data }
    }

    /// Constructs a `Fail` response with the provided data.
    ///
    /// This constructor is generic and can be used with any type `T` that implements
    /// `Debug` and `Serialize`.
    ///
    /// # Arguments
    ///
    /// * `data`: The data to include in the fail response.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use serde::Serialize;
    /// use pawz::payload::body::Body;
    ///
    /// #[derive(Serialize)]
    /// struct ErrorDetails {
    ///     reason: String,
    ///     code: u32,
    /// }
    ///
    /// let fail_response: Body<ErrorDetails> = Body::fail(ErrorDetails {
    ///     reason: "Invalid input".to_string(),
    ///     code: 400,
    /// });
    /// ```
    pub fn fail(data: T) -> Self {
        Body::Fail { data }
    }

    /// Constructs an `Error` response with a message, optional code, and optional data.
    ///
    /// This constructor allows you to create an error response that includes both
    /// a human-readable message and associated data. This can be useful for providing
    /// detailed error information to clients.
    ///
    /// # Arguments
    ///
    /// * `message`: The error message (can be converted to `String`).
    /// * `code`: An optional error code.
    /// * `data`: The optional data to include in the error response.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use serde::Serialize;
    /// use pawz::payload::body::Body;
    ///
    /// #[derive( Serialize)]
    /// struct ErrorDetails {
    ///     details: String,
    /// }
    ///
    /// let error_response: Body<ErrorDetails> = Body::error(
    ///     "Invalid input",
    ///     Some(400),
    ///     Some(ErrorDetails {
    ///         details: "Input must be a positive integer".to_string(),
    ///     }),
    /// ).unwrap();
    /// ```
    pub fn error(
        message: impl Into<String>,
        code: Option<u16>,
        data: Option<T>,
    ) -> Result<Self, Error> {
        let message: String = message.into();
        if message.trim().is_empty() {
            return Err(Error::EmptyMessage);
        }
        Ok(Body::Error {
            message,
            code,
            data,
        })
    }
}

impl Body<()> {
    /// Creates an error response without a body.
    pub fn error_no_body(message: impl Into<String>, code: Option<u16>) -> Result<Self, Error> {
        let message: String = message.into();
        if message.trim().is_empty() {
            return Err(Error::EmptyMessage);
        }
        Ok(Body::Error {
            message,
            code,
            data: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Body;
    use crate::payload::error::Error;

    use serde::Serialize;
    use serde_json::json;

    #[derive(Debug, Serialize)]
    struct SuccessResponse {
        id: String,
    }

    #[derive(Debug, Serialize)]
    struct FailResponse {
        reason: String,
    }

    #[derive(Debug, Serialize)]
    struct ErrorBody {
        stack: String,
    }

    #[test]
    fn success_response_to_json() {
        let data = SuccessResponse {
            id: "some_id".to_string(),
        };
        let response = Body::success(data);
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({ "status": "success", "data": {"id": "some_id"} })
        );
    }

    #[test]
    fn fail_response_to_json() {
        let data = FailResponse {
            reason: "test reason".to_string(),
        };

        let response = Body::fail(data);
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({ "status": "fail", "data": {"reason": "test reason"}})
        );
    }

    #[test]
    fn error_response_to_json() {
        let response = Body::error_no_body("Some problem", None).unwrap();
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({"status": "error", "message": "Some problem"})
        );
    }

    #[test]
    fn error_with_code() {
        let response = Body::error_no_body("Some problem", Some(400)).unwrap();
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({"status": "error", "message": "Some problem", "code": 400})
        );
    }

    #[test]
    fn error_with_body() {
        let body = ErrorBody {
            stack: "some stack".into(),
        };
        let response = Body::error("Some problem", None, Some(body)).unwrap();
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({"status": "error", "message": "Some problem", "data": { "stack": "some stack" }})
        );
    }

    #[test]
    fn all_properties_error() {
        let body = ErrorBody {
            stack: "some stack".into(),
        };
        let response = Body::error("Some problem", Some(400), Some(body)).unwrap();
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({"status": "error", "message": "Some problem", "data": {"stack": "some stack"}, "code": 400})
        );
    }

    #[test]
    fn empty_message_error() {
        let result = Body::error("   ", None, None::<ErrorBody>);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), Error::EmptyMessage);

        let result = Body::error_no_body("  ", None);
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), Error::EmptyMessage);
    }
}
