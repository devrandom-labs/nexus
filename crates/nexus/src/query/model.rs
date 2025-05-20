use std::{fmt::Debug, hash::Hash};

/// # `ReadModel`
///
/// A marker trait for data structures that are specifically designed for query
/// operations and read-side data representation in a CQRS architecture.
///
/// Read models are typically denormalized projections of data derived from domain
/// events or other data sources. They are optimized for efficient querying and
/// display, often tailored to specific use cases or views in an application.
///
/// This trait requires that a read model has an associated `Id` type for unique
/// identification.
///
/// Implementors must be `Send + Sync + Debug + 'static`.
pub trait ReadModel: Send + Sync + Debug + 'static {
    /// ## Associated Type: `Id`
    /// The type used to uniquely identify an instance of this read model.
    /// It must be `Send + Sync + Debug + Clone + Eq + Hash + 'static`.
    type Id: Send + Sync + Debug + Clone + Eq + Hash + 'static;
}
