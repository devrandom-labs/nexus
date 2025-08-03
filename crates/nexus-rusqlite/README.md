# nexus-rusqlite

A reference implementation of an `EventSourcedRepository` for [`nexus`](https://crates.io/crates/nexus) built on top of [`rusqlite`](https://crates.io/crates/rusqlite).

It is intended primarily for *local development* and test environments. For production workloads you will probably want to implement the repository interface for your database of choice (PostgreSQL, DynamoDB, Kafka, …).

## Quick start

```toml
[dependencies]
nexus = "0.1"
nexus-rusqlite = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use nexus_rusqlite::Store;

let store = Store::connect("sqlite://events.db").await?;
```

Migrations are located under `migrations/` and are applied automatically via [`refinery`](https://crates.io/crates/refinery).

---

Licensed under Apache-2.0. 