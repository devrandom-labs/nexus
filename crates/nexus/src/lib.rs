//! # Nexus: CQRS, ES, DDD and Hexagonal Architecture Primitives
//!
//! Nexus is a foundational Rust crate meticulously engineered to empower developers
//! in building applications that are not only **blazingly fast** and **extremely type-safe**
//! but also strictly adhere to the robust principles of Domain-Driven Design (DDD),
//! Event Sourcing (ES), and Command Query Responsibility Segregation (CQRS).
//!
//! ## Our Philosophy: Uncompromising Excellence
//!
//! We believe in providing the sharpest tools to build the most resilient and
//! performant systems. Nexus offers the core building blocks—traits, fundamental types,
//! and architectural guidance—that enable you to construct sophisticated applications.
//! While we champion architectural purity, Nexus aims to grant developers the flexibility
//! to integrate these patterns within their broader ecosystem, supporting diverse
//! operational needs such as synchronous or asynchronous command handling and the
//! use of custom or `tower`-based dispatch mechanisms.
//!
//! ## Core Architectural Pillars: CQRS & Event Sourcing
//!
//! Nexus is built upon two powerful architectural patterns:
//!
//! ### Command Query Responsibility Segregation (CQRS)
//!
//! CQRS is an architectural pattern that separates the parts of your system that
//! handle state changes (Commands) from the parts that retrieve data (Queries).
//!
//! * **What it is:** At its heart, CQRS means that operations that write data
//!   (Commands) use a different model and processing path than operations that
//!   read data (Queries). You might have a rich, behavior-driven domain model for
//!   handling commands, and a separate, optimized model (or multiple) for servicing queries.
//! * **Key Benefits:**
//!     * **Optimized Data Models:** Design write models focused on consistency and
//!       domain logic, and read models tailored for specific query needs, leading to
//!       simpler and more performant data retrieval.
//!     * **Scalability:** Scale the read and write sides of your application independently.
//!       For example, you can have many read replicas if your application is read-heavy.
//!     * **Focused Logic:** Simplifies the domain logic by separating concerns. Command
//!       handlers focus solely on executing actions and ensuring invariants, while query
//!       handlers focus on efficient data fetching.
//!
//! ### Event Sourcing (ES)
//!
//! Event Sourcing is a pattern where all changes to an application's state are
//! stored as a sequence of immutable "Domain Events." Instead of storing the current
//! state of an entity, you store the history of events that led to that state.
//!
//! * **What it is:** Every time a significant action occurs in your domain (e.g.,
//!   `UserRegistered`, `OrderPlaced`, `ItemAddedToCart`), an event is generated and
//!   persisted. The current state of an aggregate is derived by replaying these
//!   events in order.
//! * **Key Benefits:**
//!   * **Complete Audit Trail:** Provides a reliable and detailed history of all
//!     changes, crucial for auditing, debugging, and business intelligence.
//!   * **Temporal Queries:** Allows you to reconstruct the state of your system
//!     at any point in time.
//!   * **State Reconstruction & Resilience:** If a read model becomes corrupted, it
//!     can be entirely rebuilt from the event store.
//!   * **Debugging and Diagnostics:** Understanding how state evolved over time becomes
//!     much simpler.
//!   * **Decoupling:** Components can react to events, enabling a more decoupled and
//!     evolvable architecture.
//!
//! ### Domain-Driven Design (DDD)
//!
//! Domain-Driven Design is an approach to software development that emphasizes
//! a deep understanding and modeling of the core business domain. It focuses on
//! creating a rich, expressive model that captures the complexity and nuances
//! of the business logic.
//!
//! * **What it is:** DDD involves collaborating closely with domain experts to
//!   develop a "Ubiquitous Language" spoken by both technical and non-technical
//!   team members. This language is used to build a domain model, often
//!   consisting of entities, value objects, aggregates, domain events, and services,
//!   organized within "Bounded Contexts."
//! * **Key Benefits:**
//!   * **Clearer Communication:** A shared language reduces ambiguity and
//!     misunderstandings between developers and domain experts.
//!   * **Business Alignment:** Software directly reflects the business domain,
//!     making it more intuitive and fit for purpose.
//!   * **Maintainability & Evolvability:** Well-defined models and boundaries
//!     make the system easier to understand, maintain, and evolve over time.
//!   * **Reduced Complexity:** By focusing on the core domain and isolating it,
//!     complexity is managed more effectively.
//!
//! ### Hexagonal Architecture (Ports and Adapters)
//!
//! Hexagonal Architecture, also known as Ports and Adapters, is an architectural
//! pattern that aims to create loosely coupled application components by isolating
//! the core application logic from external concerns and infrastructure.
//!
//! * **What it is:** The core application logic (the "hexagon") defines "Ports"
//!   which are interfaces specifying how it interacts with the outside world
//!   (e.g., for data persistence, message queues, UI interactions). External
//!   components, or "Adapters," implement these ports to connect specific
//!   technologies (e.g., a PostgreSQL adapter for a repository port, an HTTP
//!   adapter for user interactions).
//! * **Key Benefits:**
//!   * **Testability:** The application core can be tested in isolation from
//!     infrastructure, using mock or in-memory adapters.
//!   * **Flexibility & Maintainability:** Infrastructure choices can be changed
//!     or updated without impacting the core application logic (e.g., swapping
//!     one database for another).
//!   * **Technology Agnostic Core:** The core business logic remains pure and
//!     uncluttered by specific technology details.
//!
//! ## How Nexus Empowers You
//!
//! Nexus provides the robust, type-safe, and performance-oriented primitives to
//! implement CQRS, ES, DDD, and Hexagonal Architecture effectively in Rust:
//!
//! * **Strongly-Typed Messages & Domain Primitives:** `Command`, `Query`, and
//!   `DomainEvent` traits ensure clear contracts. `AggregateRoot`, `AggregateState`,
//!   and `AggregateType` offer a structured way to model your DDD aggregates
//!   that evolve through events, forming the heart of your domain model.
//! * **Clear Boundaries for Hexagonal Architecture:** The `EventSourceRepository`
//!   and `ReadModelRepository` traits act as explicit Ports. Your application's
//!   core logic, built with Nexus, depends on these abstract interfaces. You then
//!   provide the Adapters (concrete implementations) for your chosen infrastructure,
//!   keeping the core decoupled and testable.
//! * **Performance by Design:** ... (existing text)
//! * **Flexibility with Architectural Purity:** While providing strong foundations for
//!   these architectures, Nexus remains unopinionated about ... (existing text)
//! * **`tower` Ecosystem Integration:** ... (existing text)
//!
//! ## Core Modules
//!
//! * **`command`:** This module contains all the necessary components for the write-side
//!   of your CQRS system. This includes traits and structs for defining commands,
//!   aggregates (`AggregateRoot`, `AggregateState`, `AggregateType`), command handlers
//!   (`AggregateCommandHandler`), and the event sourcing repository port
//!   (`EventSourceRepository`).
//! * **`query`:** This module provides components for the read-side. It includes
//!   traits for defining queries (`Query`), read models (`ReadModel`), query handlers
//!   (`QueryHandlerFn`), and the read model repository port (`ReadModelRepository`).
//!
//! By leveraging Nexus, you are choosing a path towards building systems that are
//! robust, maintainable, scalable, and grounded in proven architectural excellence.
//!
use serde::{Serialize, de::DeserializeOwned};
use std::{any::Any, error::Error, fmt::Debug, hash::Hash};

pub mod command;
pub mod events;
pub mod query;
pub mod store;

/// # `Message`
///
/// A common marker trait for all message types within the `nexus` system,
/// including commands, queries, and domain events.
///
/// This trait establishes a baseline for messages, requiring them to be:
/// * `Any`: Can be downcast to a concrete type.
/// * `Debug`: Can be formatted for debugging.
/// * `Send`: Can be sent safely across threads.
/// * `Sync`: Can be shared safely across threads (`&T` is `Send`).
/// * `'static`: Contains no non-static references.
///
/// Its primary role is to provide a common ancestor for different kinds of
/// messages, facilitating generic processing or type-erasure scenarios if needed,
/// though `nexus` primarily relies on strong typing via generics.
pub trait Message: Any + Debug + Send + Sync + 'static {}

/// # `Command`
///
/// Represents an intention to change the system state. Commands are imperative
/// messages that instruct the system to perform an action.
///
/// In a CQRS architecture, commands are distinct from queries. They are handled
/// by command handlers (often associated with aggregates in DDD) and typically
/// result in state changes, which are often captured as domain events in an
/// Event Sourcing model.
///
/// This trait requires `Message` and associates specific `Result` and `Error`
/// types with each command, providing a clear contract for command execution.
pub trait Command: Message {
    /// ## Associated Type: `Result`
    /// The type returned upon successful processing of the command by its handler.
    ///
    /// This could be a simple acknowledgment (e.g., `()`), an identifier (e.g., the ID
    /// of a newly created aggregate), or any other data relevant to the outcome of
    /// the command.
    ///
    /// It must be `Send + Sync + Debug + 'static`.
    type Result: Send + Sync + Debug + 'static;

    /// ## Associated Type: `Error`
    /// The specific error type returned if the command's *domain logic* fails.
    ///
    /// This error type should represent failures related to business rules, invariants,
    /// or other conditions within the domain that prevent the command from being
    /// successfully processed. It is distinct from infrastructure errors (like
    /// database connection issues or network failures), which would typically be
    /// handled at a different layer (e.g., by the repository or dispatcher).
    ///
    /// It must implement `std::error::Error` and be `Send + Sync + Debug + 'static`.
    type Error: Error + Send + Sync + Debug + 'static;
}

/// # `Query`
///
/// Represents an intention to retrieve data from the system without changing its state.
/// Queries are messages that ask the system for information.
///
/// In a CQRS architecture, queries are handled separately from commands, often
/// accessing specialized read models that are optimized for data retrieval.
///
/// This trait requires `Message` and associates specific `Result` (the data being
/// requested) and `Error` types with each query, defining a clear contract for
/// query execution.
pub trait Query: Message {
    /// ## Associated Type: `Result`
    /// The type of data returned upon successful execution of the query.
    ///
    /// This is typically a specific struct or enum representing the read model data
    /// requested by the query (e.g., `UserDetailsView`, `OrderSummaryDto`).
    ///
    /// It must be `Send + Sync + Debug + 'static`.
    type Result: Send + Sync + Debug + 'static;
    /// ## Associated Type: `Error`
    /// The specific error type returned if the query fails.
    ///
    /// This could be due to various reasons such as the requested data not being
    /// found, the requester lacking necessary permissions, or issues with the
    /// underlying read model store.
    ///
    /// It must implement `std::error::Error` and be `Send + Sync + Debug + 'static`.
    type Error: Error + Send + Sync + Debug + 'static;
}

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
pub trait DomainEvent: Message + Clone + Serialize + DeserializeOwned + PartialEq {
    /// ## Associated Type: `Id`
    /// The type of the identifier for the aggregate instance this event is associated with.
    ///
    /// This `Id` must conform to the [`Id`] trait, ensuring it has common
    /// properties like being cloneable, hashable, equatable, etc.
    type Id: Id;

    /// ## Method: `aggregate_id`
    /// Returns a reference to the unique identifier of the aggregate instance
    /// to which this event belongs.
    ///
    /// This is crucial for routing events, rehydrating aggregates, and ensuring
    /// that events are applied to the correct aggregate instance.
    fn aggregate_id(&self) -> &Self::Id;
}

/// # `Id`
///
/// A marker trait that groups common trait bounds required for types used as
/// unique identifiers within the `nexus` framework (e.g., for aggregates,
/// domain events).
///
/// This trait simplifies other trait definitions (like [`DomainEvent`] and
/// [`AggregateType`]) by providing a single name for a common set of constraints.
///
/// ## Required Bounds:
/// * `Clone`: Identifiers must be cloneable.
/// * `Send`: Identifiers must be sendable across threads.
/// * `Sync`: Identifiers must be safely shareable across threads.
/// * `Debug`: Identifiers must be debug-printable.
/// * `Hash`: Identifiers must be hashable (e.g., for use in `HashMap` keys).
/// * `Eq`: Identifiers must support equality comparison.
/// * `'static`: Identifiers must not contain any non-static references.
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + 'static {}

/// Blanket implementation of `Id`.
///
/// This implementation ensures that any type `T` which already satisfies all the
/// necessary bounds (`Clone + Send + Sync + Debug + Hash + Eq + 'static`)
/// automatically implements the `Id` marker trait. This avoids the need for
/// manual `impl Id for ...` for common types like `String`, `Uuid`, integers, etc.
impl<T> Id for T where T: Clone + Send + Sync + Debug + Hash + Eq + 'static {}
