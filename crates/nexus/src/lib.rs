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
pub mod query;

pub trait Message: Any + Debug + Send + Sync + 'static {}

/// Represents an intention to change the system state
/// It is linked to specific Result and Error types
pub trait Command: Message {
    /// The type returned upon successful processing of the command.
    type Result: Send + Sync + Debug + 'static;

    /// The specific error type returned if the command's *domain logic* fails.
    /// This is distinct  from infrastructure errors (like database connection issues).
    type Error: Error + Send + Sync + Debug + 'static;
}

/// Represents an intention to retreive data from the system without changing state.
/// It is linked to specific Result (the data requested) and Error types.
pub trait Query: Message {
    /// The type of data returned upon successful execution of the query.
    /// This is typically a specific struct or enum representing the read model data.
    type Result: Send + Sync + Debug + 'static;
    /// The specific error type returned if the query fails (e.g., data not found, access denied).
    type Error: Error + Send + Sync + Debug + 'static;
}

/// Represents a significant occurrence in the domain that has already happened.
/// Events are immutable facts.
pub trait DomainEvent: Message + Clone + Serialize + DeserializeOwned {
    type Id: Id;
    fn aggregate_id(&self) -> &Self::Id;
}

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + 'static {}

impl<T> Id for T where T: Clone + Send + Sync + Debug + Hash + Eq + 'static {}
