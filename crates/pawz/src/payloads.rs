use serde::Serialize;
use std::fmt::Debug;

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
///        must implement `Debug` and `Serialize`.
///
/// # Variants
///
/// * `Success { data: T }`: Represents a successful response containing data of type `T`.
/// * `Fail { data: T }`: Represents a failed response containing data of type `T`.
/// * `Error { message: String, code: Option<u16> }`: Represents an error response with a message
///                                                 and an optional error code.
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
/// use pawz::Response; // Assuming your enum is in a crate named 'pawz'
///
/// #[derive(Debug, Serialize)]
/// struct User {
///     id: u32,
///     name: String,
/// }
///
/// let success_response: Response<User> = Response::success(User {
///     id: 123,
///     name: "Alice".to_string(),
/// });
///
/// let error_response: Response<String> = Response::error("User not found", Some(404));
/// ```
#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub enum Response<T>
where
    T: Debug + Serialize,
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
    },
}

impl Response<String> {
    /// Constructs an `Error` response with a string message and an optional code,
    /// adhering to the JSend specification.
    ///
    /// This constructor is specialized for `Response<String>` and is useful for creating
    /// error responses with human-readable messages.
    ///
    /// # Arguments
    ///
    /// * `message`: The error message (can be converted to `String`).
    /// * `code`: An optional error code.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use pawz::Response; // Assuming your enum is in a crate named 'pawz'
    ///
    /// let error_response: Response<String> = Response::error("Database connection failed", Some(500));
    /// ```
    pub fn error(message: impl Into<String>, code: Option<u16>) -> Self {
        Response::Error {
            message: message.into(),
            code,
        }
    }
}

impl<T> Response<T>
where
    T: Debug + Serialize,
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
    /// use pawz::Response; // Assuming your enum is in a crate named 'pawz'
    ///
    /// #[derive(Debug, Serialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// let success_response: Response<User> = Response::success(User {
    ///     id: 123,
    ///     name: "Alice".to_string(),
    /// });
    /// ```
    pub fn success(data: T) -> Self {
        Response::Success { data }
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
    /// use pawz::Response; // Assuming your enum is in a crate named 'pawz'
    ///
    /// #[derive(Debug, Serialize)]
    /// struct ErrorDetails {
    ///     reason: String,
    ///     code: u32,
    /// }
    ///
    /// let fail_response: Response<ErrorDetails> = Response::fail(ErrorDetails {
    ///     reason: "Invalid input".to_string(),
    ///     code: 400,
    /// });
    /// ```
    pub fn fail(data: T) -> Self {
        Response::Fail { data }
    }
}

#[cfg(test)]
mod tests {
    use super::Response;
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

    #[test]
    fn success_response_to_json() {
        let data = SuccessResponse {
            id: "some_id".to_string(),
        };
        let response = Response::success(data);
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

        let response = Response::fail(data);
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({ "status": "fail", "data": {"reason": "test reason"}})
        );
    }

    #[test]
    fn error_response_to_json_no_code() {
        let response = Response::error("Some problem", None);
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({"status": "error", "message": "Some problem"})
        );
    }
}
