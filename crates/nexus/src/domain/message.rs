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
///
use std::{any::Any, fmt::Debug};

pub trait Message: Any + Debug + Send + Sync + 'static {}
