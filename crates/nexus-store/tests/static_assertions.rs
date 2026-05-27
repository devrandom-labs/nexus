use nexus_store::*;
use static_assertions::*;

// StoreError is a proper error (with concrete type params)
assert_impl_all!(StoreError<std::io::Error, std::io::Error, std::io::Error>: std::error::Error, Send, Sync, std::fmt::Debug);

// LoadWithError extends StoreError with the user's upcast error type
assert_impl_all!(LoadWithError<std::io::Error, std::io::Error, std::io::Error, std::convert::Infallible>: std::error::Error, Send, Sync, std::fmt::Debug);

// Envelope types are Send + Sync + Debug
assert_impl_all!(PendingEnvelope: Send, Sync, std::fmt::Debug);
assert_impl_all!(PersistedEnvelope: Send, Sync, std::fmt::Debug);

#[test]
fn static_assertions_compile() {}
