use nexus::{
    DomainEvent,
    domain::{AggregateState, DomainEvent as DomainEventTrait},
    infra::NexusId,
};
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

// ---
// 3. The Aggregate State
// ---
// The internal state of the User aggregate, built by applying events.

#[derive(Debug, PartialEq, Default)]
pub struct UserState {
    pub id: NexusId,
    pub name: String,
    pub email: String,
    pub is_active: bool,
    pub deactivation_reason: Option<String>,
}

impl UserState {
    pub fn handle_user_created(&mut self, event: &UserCreated) {
        self.id = event.user_id;
        self.name = event.name.clone();
        self.email = event.email.clone();
    }
    pub fn handle_user_name_updated(&mut self, event: &UserNameUpdated) {
        self.name = event.new_name.clone();
    }
    pub fn handle_user_activated(&mut self, _event: &UserActivated) {
        self.is_active = true;
    }
    pub fn handle_user_deactivated(&mut self, event: &UserDeactivated) {
        self.is_active = false;
        self.deactivation_reason = Some(event.reason.clone());
    }
}

impl AggregateState for UserState {
    type Domain = dyn UserEvent;
    fn apply(&mut self, event: &Self::Domain) {}
}
