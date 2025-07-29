use nexus::domain::DomainEvent;

// ---
// 1. The Bounded Context Marker Trait
// ---
// This trait logically groups all events related to the User domain.
pub trait UserEvent: DomainEvent {}
