//! # The Query Module: Reading System State
//!
//! This module provides the core components for the "read" side of a CQRS
//! (Command Query Responsibility Segregation) architecture. It focuses on how
//! data is retrieved and presented, typically from specialized read models
//! that are optimized for querying.
//!
//! In a CQRS system, the read side is separated from the write side (handled by
//! the `command` module). This separation allows for different data models,
//! performance optimizations, and scaling strategies for queries compared to commands.
//!
//! ## Key Components:
//!
//! * **`model`**: Defines the `ReadModel` trait, a marker for data structures
//!   specifically designed for query purposes.
//! * **`repository`**: Specifies the `ReadModelRepository` trait, a port for
//!   retrieving read model instances.
//! * **`handler`**: Provides `QueryHandlerFn`, a struct that adapts a function
//!   into a `tower::Service` for handling specific queries. This facilitates
//!   integration with the `tower` ecosystem for building query processing pipelines.
//!
//! The components in this module enable the creation of efficient and flexible
//! query systems, allowing applications to serve data to users or other services
//! without impacting the performance or complexity of the command-processing path.
pub mod handler;
pub mod model;
pub mod repository;

pub use handler::QueryHandlerFn;
pub use model::ReadModel;
pub use repository::ReadModelRepository;
