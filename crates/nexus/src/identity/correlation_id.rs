use serde::{Deserialize, Serialize};
use std::{fmt::Display, ops::Deref, sync::Arc};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct CorrelationId(pub Arc<String>);

impl CorrelationId {
    pub fn new(id: String) -> Self {
        CorrelationId(Arc::new(id))
    }
}

impl Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for CorrelationId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for CorrelationId {
    fn from(id: String) -> Self {
        Self(Arc::new(id))
    }
}

impl From<&str> for CorrelationId {
    fn from(id: &str) -> Self {
        Self(Arc::new(id.to_string()))
    }
}
