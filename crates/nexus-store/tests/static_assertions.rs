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

// PR2 (#208): `bytes` and the `Stream` trait are re-exported from `nexus_store`
// so downstreams share *our* version of each. These references fail to compile
// if either re-export is removed.
#[test]
fn reexports_resolve() {
    // `nexus_store::Stream` resolves and is the same trait `futures` streams
    // implement (it is `futures_core::Stream`, which `futures::Stream` re-exports).
    fn takes_stream<T: nexus_store::Stream>(_: T) {}

    // `nexus_store::bytes::Bytes` resolves (the `pub use bytes;` re-export).
    let bytes: nexus_store::bytes::Bytes = nexus_store::bytes::Bytes::new();
    assert!(bytes.is_empty());

    takes_stream(futures::stream::empty::<u8>());
}
