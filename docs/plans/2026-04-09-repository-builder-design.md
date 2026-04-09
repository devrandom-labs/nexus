# Repository Builder Design

## Goal

Replace `Store::repository(codec, upcaster)` with a typestate builder that
defaults to `JsonCodec` when the `json` feature is enabled and `()` (no-op)
for the upcaster — making the zero-config case truly zero-config:

```rust
let repo = store.repository().build();       // JsonCodec + ()
let order = repo.load(OrderId(1)).await?;    // any aggregate, inferred
```

## Architecture

A single `RepositoryBuilder<S, C, U>` type with three type parameters:

- `S` — the raw event store backend
- `C` — the codec (starts as `NeedsCodec` or `JsonCodec` depending on feature)
- `U` — the upcaster (starts as `()`)

### Typestate: preventing build without codec

`NeedsCodec` is a ZST containing `PhantomData<*const ()>`, making it `!Send`.
`build()` requires `C: Send + Sync + 'static` — a bound all real codecs
satisfy (required by `Codec<E>`), but `NeedsCodec` does not. This makes
`store.repository().build()` a compile error when no codec feature is enabled.

### Entry point

```rust
impl<S: RawEventStore> Store<S> {
    #[cfg(feature = "json")]
    pub fn repository(&self) -> RepositoryBuilder<S, JsonCodec, ()>;

    #[cfg(not(any(feature = "json")))]
    pub fn repository(&self) -> RepositoryBuilder<S, NeedsCodec, ()>;
}
```

### Transitions

```rust
impl<S, C, U> RepositoryBuilder<S, C, U> {
    pub fn codec<NewC>(self, codec: NewC) -> RepositoryBuilder<S, NewC, U>;
    pub fn upcaster<NewU>(self, upcaster: NewU) -> RepositoryBuilder<S, C, NewU>;
}
```

### Terminal methods

```rust
impl<S: RawEventStore, C: Send + Sync + 'static, U> RepositoryBuilder<S, C, U> {
    pub fn build(self) -> EventStore<S, C, U>;
    pub fn build_zero_copy(self) -> ZeroCopyEventStore<S, C, U>;
}
```

## Usage matrix

```rust
// json feature ON:
store.repository().build()                          // JsonCodec + ()
store.repository().upcaster(u).build()              // JsonCodec + u
store.repository().codec(custom).build()            // custom + ()
store.repository().codec(c).upcaster(u).build()     // c + u

// json feature OFF:
store.repository().codec(c).build()                 // c + ()
store.repository().codec(c).upcaster(u).build()     // c + u
store.repository().build()                           // COMPILE ERROR

// zero-copy:
store.repository().codec(c).build_zero_copy()       // ZeroCopyEventStore
```

## Universal repo pattern

Because `SerdeCodec<Json>` implements `Codec<E>` for all
`E: Serialize + DeserializeOwned`, a single repo serves all aggregates:

```rust
let repo = store.repository().build();
let order = repo.load(OrderId(1)).await?;   // Repository<OrderAggregate>
let user  = repo.load(UserId(42)).await?;   // Repository<UserAggregate>
```

## Breaking changes

- `Store::repository(codec, upcaster)` removed — replaced by builder
- `Store::zero_copy_repository(codec, upcaster)` removed — replaced by `build_zero_copy()`

## File plan

- Create: `crates/nexus-store/src/store/builder.rs`
- Modify: `crates/nexus-store/src/store/store.rs` (replace methods with builder entry point)
- Modify: `crates/nexus-store/src/store/mod.rs` (add module, re-export)
- Modify: `crates/nexus-store/src/lib.rs` (re-export `RepositoryBuilder`)
- Update: all tests and examples that use the old API
