use serde::{Deserialize, Serialize};
use std::{default::Default, fmt::Display, ops::Deref};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EventId(Uuid);

impl EventId {
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub fn into_inner(self) -> Uuid {
        self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        EventId(Uuid::now_v7())
    }
}

impl Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for EventId {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Uuid> for EventId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl From<EventId> for Uuid {
    fn from(value: EventId) -> Self {
        value.0
    }
}

#[cfg(feature = "testing")]
impl fake::Dummy<fake::Faker> for EventId {
    fn dummy_with_rng<R: fake::Rng + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        let bytes = rng.random::<[u8; 16]>();
        let uuid = uuid::Builder::from_random_bytes(bytes)
            .with_variant(uuid::Variant::RFC4122)
            .with_version(uuid::Version::SortRand)
            .into_uuid();
        EventId(uuid)
    }
}
