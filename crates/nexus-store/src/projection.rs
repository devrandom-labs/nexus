use nexus::DomainEvent;

/// A pure fold function over domain events.
///
/// Processes events one at a time to produce derived state. The
/// framework handles all IO (reading events, persisting state,
/// checkpointing). The projector is only responsible for computation.
///
/// Fallible: `apply` returns `Result` because projections may perform
/// checked arithmetic or encounter domain-specific edge cases.
/// Recovery policy (skip, fail, dead-letter) is handled by middleware
/// layers, not the projector itself.
///
/// # Comparison with `AggregateState`
///
/// `AggregateState::apply` is infallible because an aggregate always
/// applies its own events. A projector may process events from any
/// source, and may do derived computations (sums, counts) that can
/// overflow.
pub trait Projector: Send + Sync + 'static {
    /// The domain event type this projector handles.
    type Event: DomainEvent;

    /// The derived state produced by folding events.
    type State: Send + Sync + 'static;

    /// Error type for fallible projection logic.
    type Error: core::error::Error + Send + Sync + 'static;

    /// The initial state before any events have been applied.
    fn initial(&self) -> Self::State;

    /// Apply a single event to the current state, producing new state.
    ///
    /// Must use checked arithmetic for all computations. Return `Err`
    /// on overflow, underflow, or any domain-specific invariant
    /// violation. The framework decides recovery policy.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the event cannot be applied — e.g.,
    /// arithmetic overflow, underflow, or domain invariant violation.
    fn apply(&self, state: Self::State, event: &Self::Event) -> Result<Self::State, Self::Error>;
}
