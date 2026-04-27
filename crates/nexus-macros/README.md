# nexus-macros

Procedural macros for [`nexus`](../nexus). Three macros, zero boilerplate.

## `#[derive(DomainEvent)]`

Derive on an enum. Generates `Message` + `DomainEvent` impls with `name()` returning variant names as `&'static str`.

```rust
#[derive(Debug, Clone, DomainEvent)]
enum AccountEvent {
    Opened(AccountOpened),
    Deposited(MoneyDeposited),
    Closed(AccountClosed),
}
```

## `#[nexus::aggregate]`

Attribute macro on a unit struct. Generates `Aggregate` impl, `AggregateEntity` impl, newtype wrapping `AggregateRoot`, `new(id)` constructor, and redacted `Debug`.

```rust
#[nexus::aggregate(state = AccountState, error = AccountError, id = AccountId)]
struct BankAccount;
```

## `#[nexus::transforms]`

Attribute macro on an impl block. Generates an `Upcaster` impl for schema evolution. Transform functions are annotated with `#[transform(event = "...", from = N, to = N+1)]`.

```rust
#[nexus::transforms(aggregate = BankAccount, error = MyUpcastError)]
impl BankAccountTransforms {
    #[transform(event = "Deposited", from = 1, to = 2)]
    fn add_currency(payload: &[u8]) -> Result<Vec<u8>, MyUpcastError> {
        // migrate v1 → v2
    }
}
```

## License

Licensed under your choice of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE).
