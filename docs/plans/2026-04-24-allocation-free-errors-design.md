# Allocation-Free Error Paths

**Issue:** [#152](https://github.com/devrandom-labs/nexus/issues/152)
**Date:** 2026-04-24

## Problem

Error types in `nexus-store` and `nexus-fjall` heap-allocate in 6+ places via `Box<dyn Error>` and `String`. On IoT/embedded targets under memory pressure, error paths should not allocate. The `bounded-labels` feature on `StreamLabel` solved one allocation while leaving every other error variant allocating — an inconsistent half-measure.

## Design

### `StoreError<A, C, U>` — three type parameters, maximum precision

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError<A, C, U> {
    Conflict {
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    StreamNotFound { stream_id: ArrayString<64> },
    Adapter(A),
    Codec(C),
    Upcast(UpcastError<U>),
    Kernel(KernelError),
    VersionOverflow,
}
```

- `A`: adapter error (`FjallError`, etc.)
- `C`: codec error (`Codec::Error`, `BorrowingCodec::Error`)
- `U`: upcaster transform error (user-defined)
- `VersionOverflow`: replaces `Codec("version overflow...".into())` — honest variant, zero-size
- `ArrayString<64>` from `arrayvec` replaces `StreamLabel` — stack-allocated UTF-8

### `AppendError<A>` — unchanged shape, `ArrayString<64>` for stream ID

```rust
#[derive(Debug)]
pub enum AppendError<A> {
    Conflict {
        stream_id: ArrayString<64>,
        expected: Option<Version>,
        actual: Option<Version>,
    },
    Store(A),
}
```

### `UpcastError<U>` — generic over transform error

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum UpcastError<U> {
    VersionNotAdvanced {
        event_type: ArrayString<64>,
        input_version: Version,
        output_version: Version,
    },
    EmptyEventType {
        input_event_type: ArrayString<64>,
        schema_version: Version,
    },
    ChainLimitExceeded {
        event_type: ArrayString<64>,
        schema_version: Version,
        limit: u64,
    },
    TransformFailed {
        event_type: ArrayString<64>,
        schema_version: Version,
        source: U,
    },
}
```

- `event_type: String` → `ArrayString<64>`. Event type names from `DomainEvent::name()` are short `&'static str` variant names.
- `source: Box<dyn Error>` → `source: U`. Concrete transform error, zero allocation.

### `Upcaster` trait — associated `Error` type

```rust
pub trait Upcaster: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError<Self::Error>>;
}
```

The `()` no-op impl sets `type Error = Infallible`.

### `FjallError` — allocation-free adapter errors

```rust
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FjallError {
    Io(#[from] fjall::Error),
    CorruptValue {
        stream_id: ArrayString<64>,
        version: Option<u64>,
    },
    CorruptMeta { stream_id: ArrayString<64> },
    InvalidInput {
        stream_id: ArrayString<64>,
        version: u64,
        reason: ArrayString<128>,
    },
    VersionOverflow,
}
```

`fjall::Error` is third-party — may allocate internally. Boundary: our code never allocates on error paths.

### `Id::to_label()` — default method on the kernel trait

```rust
pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {
    const BYTE_LEN: usize;

    fn to_label(&self) -> ArrayString<64> {
        use std::fmt::Write;
        let mut buf = ArrayString::<64>::new();
        let _ = write!(buf, "{self}");
        buf
    }
}
```

`arrayvec` is already a kernel dependency. Every `Id` impl gets `to_label()` for free. Replaces `StreamLabel`, `ToStreamLabel`, and the `bounded-labels` feature.

### Error propagation

```
RawEventStore::append()           -> AppendError<S::Error>
EventStore<S,C,U>::save()         -> StoreError<S::Error, C::Error, U::Error>
Repository<A>::Error              = StoreError<S::Error, C::Error, U::Error>
```

`Repository` keeps its associated type — no erasure, no downcasting. Callers see the concrete error tree through `type Error`.

### Deletions

- `stream_label.rs` — entire file
- `StreamLabel` type — both `cfg` variants
- `ToStreamLabel` trait — blanket impl
- `bounded-labels` feature flag — from `Cargo.toml`
- `stream_label_tests.rs` — entire test file
- `pub mod stream_label` and `pub use` in `lib.rs`

### Dependency changes

- `nexus-store/Cargo.toml`: add `arrayvec = { workspace = true }`, remove `bounded-labels` feature
- `nexus-fjall/Cargo.toml`: add `arrayvec = { workspace = true }`
- Run `cargo hakari generate` after
