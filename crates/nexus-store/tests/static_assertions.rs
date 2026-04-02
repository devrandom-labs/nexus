use nexus_store::*;
use static_assertions::*;

// StoreError is a proper error
assert_impl_all!(StoreError: std::error::Error, Send, Sync, std::fmt::Debug);

// Envelope types are Send + Sync + Debug
assert_impl_all!(PendingEnvelope<()>: Send, Sync, std::fmt::Debug);
assert_impl_all!(PersistedEnvelope<'static, ()>: Send, Sync, std::fmt::Debug);

#[test]
fn static_assertions_compile() {}
