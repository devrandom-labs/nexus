/// # `DomainEvent`
///
/// Represents a significant occurrence or fact within the domain that has already
/// happened. Events are immutable and capture a specific state change or
/// noteworthy moment in the lifecycle of an aggregate or the system.
///
/// In an Event Sourcing system, domain events are the primary source of truth.
/// The state of an aggregate is derived by applying a sequence of its domain events.
///
/// ## Trait Bounds:
/// * Requires [`Message`] as a base.
/// * `Clone`: Events must be cloneable.
/// * `Serialize` and `DeserializeOwned` (from `serde`): Events must be serializable
///   and deserializable, as they are intended to be persisted in an event store.
/// * `PartialEq`: Events must be comparable for equality, primarily for testing purposes
///   (e.g., asserting that expected events were generated).
///
/// ## Associated Types:
/// * `Id`: The type of the identifier for the aggregate instance to which this event pertains.
///   This `Id` type must itself implement the [`Id`] marker trait.
///
use super::message::Message;
use serde::{Serialize, de::DeserializeOwned};

pub trait DomainEvent: Message + Clone + Serialize + DeserializeOwned + PartialEq {
    fn name(&self) -> &'static str;
}
