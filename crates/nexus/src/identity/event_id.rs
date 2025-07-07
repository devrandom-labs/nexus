use serde::{Deserialize, Serialize};
use std::{default::Default, fmt::Display, ops::Deref};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EventRecordId(Uuid);

impl EventRecordId {
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub fn into_inner(self) -> Uuid {
        self.0
    }
}

impl Default for EventRecordId {
    fn default() -> Self {
        EventRecordId(Uuid::now_v7())
    }
}

impl Display for EventRecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for EventRecordId {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Uuid> for EventRecordId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl From<EventRecordId> for Uuid {
    fn from(value: EventRecordId) -> Self {
        value.0
    }
}
