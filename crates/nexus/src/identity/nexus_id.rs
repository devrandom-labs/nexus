use crate::core::Id;
use uuid::Uuid;

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct NexusId(Uuid);

impl Default for NexusId {
    fn default() -> Self {
        NexusId(Uuid::new_v4())
    }
}

impl ToString for NexusId {
    fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl AsRef<[u8]> for NexusId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for NexusId {}
