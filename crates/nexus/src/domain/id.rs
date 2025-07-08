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
/// * `ToString`: Identifiers must support equality comparison.
/// * `AsRef[u8]`: Identifiers must not contain any non-static references.
///
use std::{fmt::Debug, hash::Hash};
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + 'static + ToString + AsRef<[u8]> {}
