#![allow(
    clippy::result_large_err,
    reason = "FjallError is intentionally stack-allocated (~208 bytes) for IoT targets"
)]

pub mod builder;
pub mod encoding;
pub mod error;
mod partition;
pub mod store;
pub mod stream;
mod subscription_stream;

pub use builder::FjallStoreBuilder;
pub use error::FjallError;
pub use partition::KeyspaceConfig;
pub use store::FjallStore;
pub use stream::FjallStream;
pub use subscription_stream::FjallSubscriptionStream;
