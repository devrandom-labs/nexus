mod builder;
mod error;
mod persist;
#[allow(
    clippy::module_inception,
    reason = "ProjectionRunner is the primary type of this module"
)]
mod runner;

pub use builder::ProjectionRunnerBuilder;
pub use error::{ProjectionError, StatePersistError};
pub use persist::{NoStatePersistence, StatePersistence, WithStatePersistence};
pub use runner::ProjectionRunner;
