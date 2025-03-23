use serde::Serialize;
use std::fmt::Debug;

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
    Error { message: String, code: Option<u16> },
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
        let response = Response::Success { data };
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

        let response = Response::Fail { data };
        let serialized = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serialized,
            json!({ "status": "fail", "data": {"reason": "test reason"}})
        );
    }
}
