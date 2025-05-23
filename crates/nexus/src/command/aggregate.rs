use super::handler::{AggregateCommandHandler, CommandHandlerResponse};
use crate::{Command, DomainEvent, Id};
use std::fmt::Debug;
use thiserror::Error as ThisError;

/// # `AggregateState<E>`
///
/// Represents the actual state data of an aggregate.
///
/// In an Event Sourcing model, the state of an aggregate is rebuilt by applying a
/// sequence of domain events. This trait ensures that the state knows how to
/// evolve itself in response to these events. The state should be simple data
/// and should not contain complex business logic beyond direct state mutation.
///
/// Implementors should be `Default` to represent the initial state of an aggregate
/// before any events have been applied.
pub trait AggregateState: Default + Send + Sync + Debug + 'static {
    /// ## Associated Type: `Event`
    /// The specific type of [`DomainEvent`] this aggregate state reacts to.
    type Event: DomainEvent;

    /// ## Method: `apply`
    /// Mutates the state based on a received domain event.
    ///
    /// This method is the core of state reconstruction in Event Sourcing. It is called
    /// both when rehydrating an aggregate from its history and when applying newly
    /// generated events from a command execution.
    ///
    /// **Important:** This method should *not* contain complex business logic, validation,
    /// or decision-making. Its sole responsibility is to change the state fields based
    /// on the data within the event. All decisions and validations should occur within
    /// the command handler *before* events are generated.
    fn apply(&mut self, event: &Self::Event);
}

/// # `AggregateType`
///
/// A marker trait used to define the specific component types associated with a
/// particular kind of aggregate (e.g., `UserAggregate`, `OrderAggregate`).
///
/// This trait acts as a central point of configuration for an aggregate, linking
/// together its unique identifier type, its specific domain event type, and its
/// state representation. It is typically implemented on a distinct, often empty,
/// type (like a unit struct) for each kind of aggregate in your domain.
///
/// This compile-time linkage ensures type safety and consistency across the
/// different parts of the aggregate's lifecycle (loading, command execution, saving).
pub trait AggregateType: Send + Sync + Debug + Copy + Clone + 'static {
    /// ## Associated Type: `Id`
    /// The type used to uniquely identify an instance of this aggregate.
    /// Must be cloneable, debuggable, hashable, equatable, and representable as a string.
    type Id: Id;

    // ## Associated Type: `Event`
    /// The specific type of [`DomainEvent`] associated with this aggregate.
    /// This event type must itself implement [`DomainEvent`] with its `Id` associated
    /// type matching `Self::Id`.
    type Event: DomainEvent<Id = Self::Id>;

    /// ## Associated Type: `State`
    /// The specific type representing the internal state data of this aggregate.
    /// This state type must implement [`AggregateState`], and crucially, its
    /// `Event` associated type must match `Self::Event`. This ensures that the
    /// aggregate's state can only be mutated by its own defined event type.
    type State: AggregateState<Event = Self::Event>;
}

/// # `Aggregate`
///
/// Provides a standard interface for accessing common properties of an aggregate instance.
/// This trait is primarily implemented by [`AggregateRoot`] to expose its core characteristics.
///
/// This allows generic code to interact with different aggregate roots in a uniform way
/// for read-only purposes like accessing ID, version, and current state, or for taking
/// ownership of uncommitted events.
pub trait Aggregate: Debug + Send + Sync + 'static {
    /// ## Associated Type: `Id`
    /// The type used to uniquely identify this aggregate instance, inherited from `AggregateType::Id`.
    type Id: Id;

    /// ## Associated Type: `Event`
    /// The specific type of [`DomainEvent`] associated with this aggregate, inherited from `AggregateType::Event`.
    type Event: DomainEvent<Id = Self::Id>;

    /// ## Associated Type: `State`
    /// The specific type representing the internal state data, inherited from `AggregateType::State`.
    type State: AggregateState<Event = Self::Event>;

    /// ## Method: `id`
    /// Returns a reference to the aggregate's unique identifier.
    fn id(&self) -> &Self::Id;

    /// ## Method: `version`
    /// Returns the version of the aggregate as loaded from the event store.
    /// This represents the number of events that have been historically applied to
    /// build the current state *before* any new command executions. It is crucial
    /// for optimistic concurrency checks when saving the aggregate.
    fn version(&self) -> u64;

    /// ## Method: `state`
    /// Returns a reference to the current state of the aggregate.
    fn state(&self) -> &Self::State;

    /// ## Method: `take_uncommitted_events`
    /// Takes ownership of newly generated events that have resulted from command
    /// execution but have not yet been persisted.
    ///
    /// This method is typically called by the [`EventSourceRepository`] during the save
    /// process. After calling this, the aggregate's internal list of uncommitted events
    /// will be empty.
    fn take_uncommitted_events(&mut self) -> Vec<Self::Event>;
}

/// # `AggregateLoadError<Id>`
///
/// Represents errors that can occur specifically during the process of loading an
/// aggregate's state from its event history using [`AggregateRoot::load_from_history`].
///
/// The generic parameter `Id` is the type of the aggregate's identifier.
#[derive(Debug, ThisError, PartialEq)]
pub enum AggregateLoadError<Id>
where
    Id: Debug,
{
    /// ## Variant: `MismatchedAggregateId`
    /// Indicates an integrity error where an event loaded from the history for a
    /// specific aggregate ID (`expected_id`) was found to have a different
    /// aggregate ID (`event_aggregate_id`) within its own data. This suggests
    /// a potential corruption or incorrect data retrieval from the event store.
    #[error(
        "Event integrity error: Loaded event with mismatched aggregate ID. Expected '{expected_id:?}', found in event: '{event_aggregate_id:?}'."
    )]
    MismatchedAggregateId {
        /// The ID of the aggregate we are attempting to load.
        expected_id: Id,
        /// The ID found within the problematic event.
        event_aggregate_id: Id,
    },
}

/// # `AggregateRoot<AT>`
///
/// The concrete implementation that manages an aggregate's state based on a sequence
/// of domain events. It encapsulates the core logic for an event-sourced entity.
///
/// The `AggregateRoot` is generic over `AT`, which must implement the [`AggregateType`]
/// trait. This `AT` defines the specific `Id`, `Event`, and `State` types for this
/// kind of aggregate, ensuring type safety and consistency.
///
/// ## Responsibilities:
/// * Holding the aggregate's unique identifier (`id`).
/// * Maintaining the current `state` of the aggregate.
/// * Tracking the `version` (number of events applied from history).
/// * Storing `uncommitted_events` generated by command execution.
/// * Providing methods to create a new aggregate (`new`).
/// * Rehydrating its state from historical events (`load_from_history`).
/// * Executing commands against its current state via an [`AggregateCommandHandler`] (`execute`).
#[derive(Debug)]
pub struct AggregateRoot<AT: AggregateType> {
    id: AT::Id,
    state: AT::State,
    version: u64,
    uncommitted_events: Vec<AT::Event>,
}

impl<AT> Aggregate for AggregateRoot<AT>
where
    AT: AggregateType,
{
    type Id = AT::Id;
    type Event = AT::Event;
    type State = AT::State;

    /// Returns a reference to the aggregate's unique identifier.
    fn id(&self) -> &Self::Id {
        &self.id
    }

    /// Returns a reference to the current state of the aggregate.
    fn state(&self) -> &Self::State {
        &self.state
    }

    /// Returns the version of the aggregate as loaded from the event store.
    /// This represents the number of events that have been historically applied to
    /// build the current state *before* any new command executions.
    fn version(&self) -> u64 {
        self.version
    }

    /// Takes ownership of newly generated events.
    /// After calling this, the aggregate's internal list of uncommitted events will be empty.
    fn take_uncommitted_events(&mut self) -> Vec<Self::Event> {
        std::mem::take(&mut self.uncommitted_events)
    }
}

impl<AT> AggregateRoot<AT>
where
    AT: AggregateType,
{
    /// # Method: `new`
    /// Creates a new `AggregateRoot` instance with a given `id`.
    ///
    /// The aggregate's state is initialized to `AT::State::default()`, its version is set to `0`,
    /// and it starts with no uncommitted events. This represents an aggregate that has just
    /// been created and has not yet had any events applied or commands processed that
    /// would generate initial events (initial events typically come from handling a creation command).
    pub fn new(id: AT::Id) -> Self {
        Self {
            id,
            state: AT::State::default(),
            version: 0,
            uncommitted_events: Vec::new(),
        }
    }

    /// # Method: `load_from_history`
    /// Rehydrates the aggregate's state by applying an iterator of historical events.
    ///
    /// This method takes a unique `id` and an `history` (an iterator over `AT::Event`).
    /// It initializes a default state, then iterates through the provided events,
    /// applying each one to the state using `AT::State::apply`.
    ///
    /// The version of the aggregate is incremented for each event successfully applied.
    ///
    /// ## Errors
    /// Returns [`AggregateLoadError::MismatchedAggregateId`] if any event in the
    /// history has an `aggregate_id()` that does not match the provided `id`
    /// for this aggregate root. This ensures data integrity.
    pub fn load_from_history(
        id: AT::Id,
        history: impl IntoIterator<Item = AT::Event>,
    ) -> Result<Self, AggregateLoadError<AT::Id>>
    where
        AT::Id: ToString,
    {
        let mut state = AT::State::default();
        let mut version = 0u64;

        for event in history {
            let event_id = event.aggregate_id();
            if event_id != &id {
                return Err(AggregateLoadError::MismatchedAggregateId {
                    expected_id: id,
                    event_aggregate_id: event_id.clone(),
                });
            }

            state.apply(&event);
            version += 1;
        }

        Ok(Self {
            id,
            state,
            version,
            uncommitted_events: Vec::new(),
        })
    }

    /// # Method: `current_version`
    /// Gets the effective current version of the aggregate, including any uncommitted events.
    ///
    /// This is calculated as the `version` loaded from history plus the number of
    /// `uncommitted_events` generated by the most recent command execution.
    /// This value is typically used by an [`EventSourceRepository`] when saving the
    /// aggregate to implement optimistic concurrency control. The repository would expect
    /// this `current_version` to match the version of the last event being appended to the stream.
    pub fn current_version(&self) -> u64 {
        self.version + self.uncommitted_events.len() as u64
    }

    /// # Method: `execute`
    /// Executes a given `command` against the aggregate's current state using the provided `handler`.
    ///
    /// This is the primary method for an aggregate to process intentions to change its state.
    ///
    /// ## Type Parameters:
    /// * `C`: The type of the command, which must implement [`Command`].
    /// * `Handler`: The type of the command handler, which must implement
    ///   [`AggregateCommandHandler<C, Services, AggregateType = AT>`]. This ensures the handler
    ///   is specific to this command type, this aggregate type, and can use the provided `Services`.
    /// * `Services`: A type (often a trait object or a struct of services) that provides
    ///   any external dependencies the `handler` might need to process the command
    ///   (e.g., a validation service, a service to fetch external data, etc.).
    ///   It must be `Send + Sync + ?Sized`.
    ///
    /// ## Process:
    /// 1.  The `command` is passed to the `handler.handle()` method along with the
    ///     current `state` of the aggregate and the `services`.
    /// 2.  If the handler succeeds, it returns a [`CommandHandlerResponse`] containing
    ///     a list of new domain events and a result value (`C::Result`).
    /// 3.  Each of these new events is then applied to the aggregate's `state` using `self.state.apply()`.
    /// 4.  The new events are added to the `uncommitted_events` list.
    /// 5.  The `result` from the command handler is returned.
    ///
    /// ## Returns:
    /// * `Ok(C::Result)` if the command is processed successfully by the handler and all
    ///   resulting events are applied.
    /// * `Err(C::Error)` if the command handler returns an error.
    pub async fn execute<C, Handler, Services>(
        &mut self,
        command: C,
        handler: &Handler,
        services: &Services,
    ) -> Result<C::Result, C::Error>
    where
        C: Command,
        Handler: AggregateCommandHandler<C, Services, AggregateType = AT>,
        Services: Send + Sync + ?Sized,
    {
        let CommandHandlerResponse { events, result } =
            handler.handle(&self.state, command, services).await?;
        for event in &events {
            self.state.apply(event);
        }
        self.uncommitted_events.extend(events);
        Ok(result)
    }
}

#[cfg(test)]
pub mod test {
    use super::super::test::{
        CreateUser, CreateUserHandler, User, UserDomainEvents, UserError, UserState,
        utils::{EventType, get_user_events},
    };
    use super::{Aggregate, AggregateLoadError, AggregateRoot, AggregateState};
    use chrono::Utc;

    #[test]
    fn user_state_default() {
        let user_state = UserState::default();
        assert_eq!(user_state.email, None);
        assert_eq!(user_state.created_at, None);
        assert!(!user_state.is_active);
    }

    #[test]
    fn user_state_apply_created() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        let user_created = UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp,
        };

        user_state.apply(&user_created);

        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
    }

    #[test]
    fn user_state_apply_activated() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        for events in get_user_events(Some(timestamp), EventType::Ordered) {
            user_state.apply(&events);
        }
        assert!(user_state.is_active);
    }

    #[test]
    fn user_state_apply_order() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        for events in get_user_events(Some(timestamp), EventType::UnOrdered) {
            user_state.apply(&events);
        }
        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
        assert!(!user_state.is_active);
    }

    #[test]
    fn user_state_apply_idempotency() {
        let mut user_state = UserState::default();
        let timestamp = Utc::now();
        let user_created = UserDomainEvents::UserCreated {
            id: "id".to_string(),
            email: String::from("joel@tixlys.com"),
            timestamp,
        };
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        user_state.apply(&user_created);
        assert_eq!(user_state.email, Some(String::from("joel@tixlys.com")));
        assert_eq!(user_state.created_at, Some(timestamp));
        assert!(!user_state.is_active);
    }

    #[test]
    fn aggregate_root_new() {
        let mut root = AggregateRoot::<User>::new(String::from("id"));
        assert_eq!(root.id(), "id");
        assert_eq!(root.version(), 0);
        assert_eq!(root.state(), &UserState::default());
        assert_eq!(root.current_version(), 0);
        assert_eq!(root.take_uncommitted_events(), Vec::new());
    }

    #[test]
    fn aggregate_root_load_from_history() {
        let timestamp = Utc::now();
        let history = get_user_events(Some(timestamp), EventType::Ordered);
        let aggregate_root = AggregateRoot::<User>::load_from_history(String::from("id"), history);
        assert!(aggregate_root.is_ok());
        let mut aggregate_root = aggregate_root.unwrap();
        assert_eq!(aggregate_root.id(), "id");
        assert_eq!(aggregate_root.version(), 2);
        assert_eq!(
            aggregate_root.state(),
            &UserState::new(Some(String::from("joel@tixlys.com")), true, Some(timestamp))
        );
        assert_eq!(aggregate_root.current_version(), 2);
        assert_eq!(aggregate_root.take_uncommitted_events(), Vec::new());
    }

    #[test]
    fn aggregate_root_load_fail() {
        let timestamp = Utc::now();
        let history = get_user_events(Some(timestamp), EventType::Ordered);
        let aggregate_root =
            AggregateRoot::<User>::load_from_history(String::from("wrong_id"), history);
        assert!(aggregate_root.is_err());
        let error = aggregate_root.unwrap_err();

        assert_eq!(
            error,
            AggregateLoadError::MismatchedAggregateId {
                expected_id: "wrong_id".to_string(),
                event_aggregate_id: "id".to_string()
            }
        );
    }

    #[tokio::test]
    async fn aggregate_root_execute_success() {
        let mut root = AggregateRoot::<User>::new(String::from("id"));
        let create_user = CreateUser {
            user_id: "id".to_string(),
            email: "joel@tixlys.com".to_string(),
        };
        let handler = CreateUserHandler;
        let result = root.execute(create_user, &handler, &()).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result, "id");
    }

    #[tokio::test]
    async fn aggregate_root_domain_error() {
        let mut root = AggregateRoot::<User>::new(String::from("id"));
        let create_user = CreateUser {
            user_id: "id".to_string(),
            email: "error@tixlys.com".to_string(),
        };
        let handler = CreateUserHandler;
        let result = root.execute(create_user, &handler, &()).await;
        assert!(result.is_err());
        let result = result.unwrap_err();
        assert_eq!(result, UserError::FailedToCreateUser);
    }
}
