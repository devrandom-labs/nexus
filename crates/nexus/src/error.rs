use thiserror::Error;
use tower::BoxError;

/// The primary, top-level error type for the `nexus` crate.
///
/// This enum serves as the main error contract for high-level operations,
/// providing a unified interface for failures that can occur across different
/// architectural layers (e.g., storage, repository logic, serialization).
///
/// # Architectural Approach: The Hierarchical Facade
///
/// This enum follows a "Hierarchical Facade" or "Error Promotion" pattern.
/// The goal is to provide a user-friendly API that avoids forcing consumers
/// into verbose nested `match` statements for common, actionable errors, while
/// still maintaining a clean, modular, and decoupled internal error structure.
///
/// This is achieved by having two categories of variants:
///
/// 1.  **Promoted Variants**: These are for the most common and actionable errors
///     that a consumer would likely want to handle programmatically (e.g.,
///     `ConnectionFailed`, `ConcurrencyConflict`). These errors are "promoted"
///     from lower-level, co-located error enums (like `store::Error`) into
///     first-class variants here.
///
/// 2.  **Wrapper Variants**: These act as a "catch-all" for less common or more
///     technical errors from a specific architectural layer (e.g., `Store`).
///     This prevents the top-level `Error` from becoming a bloated "God Object"
///     that knows about every possible failure in the system, preserving modularity.
///
/// The `From` trait is implemented to intelligently triage errors from lower
/// layers, mapping them to either a promoted variant or a wrapper variant as
/// appropriate. This makes error propagation with the `?` operator seamless.
#[derive(Debug, Error)]
pub enum Error {
    /// A fundamental infrastructure error indicating that a connection to the
    /// underlying data store could not be established or was lost.
    ///
    /// This is a **promoted** error from the storage layer, elevated to the top
    /// level because it represents a common, actionable failure that consumers
    /// may wish to handle with specific logic, such as a retry policy or a
    /// circuit breaker.
    #[error("A connection to the data store could not be established")]
    ConnectionFailed {
        #[source]
        source: BoxError,
    },

    /// A required source for data was not found.
    ///
    /// This is a **promoted** error from the storage layer, typically indicating
    /// that a required database table, collection, or document store does not
    /// exist. It is considered an actionable failure, as a consumer might handle
    /// it by triggering a setup routine or database migrations.
    #[error("Source '{name}' not found (e.g., Table, Collection, Document)")]
    SourceNotFound { name: String },

    #[error("A stream with ID '{id}' already exists, violating a unique constraint")]
    UniqueIdViolation { id: String },

    /// A wrapper for any other error originating from the storage layer (`store::Error`)
    /// that has not been promoted to a first-class variant on this top-level enum.
    ///
    /// This variant acts as a "catch-all" for less common or purely technical
    /// storage errors. This preserves the full error detail for logging and
    /// debugging without bloating the top-level API. Consumers will typically
    /// log this error rather than matching on its inner content.
    #[error(transparent)]
    Store {
        #[from]
        source: BoxError,
    },

    /// An error occurred while deserializing event data from a raw format (e.g., JSON)
    /// into a structured `EventRecord`.
    ///
    /// This is treated as a top-level, cross-cutting concern because deserialization
    /// can occur at multiple points when interacting with infrastructure.
    #[error("Failed to deserialize event data")]
    DeserializationError {
        #[source]
        source: BoxError,
    },

    /// An error occurred while serializing an `EventRecord` into a raw format for persistence.
    ///
    /// Like deserialization, this is a top-level concern as it is a fundamental
    /// infrastructure operation.
    #[error("Failed to serialize event data")]
    SerializationError {
        #[source]
        source: BoxError,
    },

    /// ## Variant: `Conflict`
    /// Signals an optimistic concurrency control violation. This typically happens when
    /// attempting to save an aggregate, and its expected version (based on when it was loaded)
    /// does not match the current version in the event store, indicating that another
    /// process has modified the aggregate in the meantime.
    #[error(
        "Concurrency conflict for stream '{stream_id:?}'. Expected version {expected_version}."
    )]
    Conflict {
        stream_id: String,
        expected_version: u64,
    },

    #[error("System Error: '{reason:?}'")]
    System { reason: String },
}
