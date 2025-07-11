use super::message::Message;
use serde::{Serialize, de::DeserializeOwned};

pub trait DomainEvent: Message + Clone + Serialize + DeserializeOwned + PartialEq {
    fn name(&self) -> &'static str;
}
