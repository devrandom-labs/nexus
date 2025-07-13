use std::{fmt::Display, str::FromStr};

use crate::domain::Id;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Eq, Hash, PartialEq, Copy, Serialize, Deserialize)]
pub struct NexusId(Uuid);

impl Default for NexusId {
    fn default() -> Self {
        NexusId(Uuid::new_v4())
    }
}

impl Display for NexusId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

impl FromStr for NexusId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(s)?;
        Ok(NexusId(uuid))
    }
}

impl AsRef<[u8]> for NexusId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl From<Uuid> for NexusId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl Id for NexusId {}
