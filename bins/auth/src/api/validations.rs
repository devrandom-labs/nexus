use crate::commons::password_validation::validate;
use serde_json::json;
use validator::ValidationError;

pub fn validate_password(password: &str) -> Result<(), ValidationError> {
    validate(password).map_err(|err| {
        let mut error = ValidationError::new("password_policy");
        for (index, err) in err.0.iter().enumerate() {
            error.add_param(index.to_string().into(), &json!(err.to_string()));
        }
        error
    })
}
