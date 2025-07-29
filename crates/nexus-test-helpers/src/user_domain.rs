use nexus::{DomainEvent, domain::DomainEvent as DomainEventTrait, infra::NexusId};
use serde::{Deserialize, Serialize};

// ---
// 1. The Bounded Context Marker Trait
// ---
// This trait logically groups all events related to the User domain.
pub trait UserEvent: DomainEventTrait {}

// ---
// 2. The Test Domain Events
// ---
// A variety of structs to represent different kinds of state changes.
/// Event for when a user is first created. Contains the initial data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "user_created")]
pub struct UserCreated {
    pub user_id: NexusId,
    pub name: String,
    pub email: String,
}
impl UserEvent for UserCreated {}

/// Event for when a user's name is updated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "user_name_updated")]
pub struct UserNameUpdated {
    pub new_name: String,
}
impl UserEvent for UserNameUpdated {}

/// Event for when a user is activated. A simple "tag-like" event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "user_activated")]
pub struct UserActivated;
impl UserEvent for UserActivated {}

/// Event for when a user is deactivated, which requires a reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "user_deactivated")]
pub struct UserDeactivated {
    pub reason: String,
}

impl UserEvent for UserDeactivated {}
