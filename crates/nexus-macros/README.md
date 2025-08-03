# nexus-macros

Procedural macros that eliminate boilerplate when using [`nexus`](https://crates.io/crates/nexus).

Currently exported derives:

* `Command`
* `Query`
* `DomainEvent`

## Example

```rust
use nexus_macros::{Command, DomainEvent};

#[derive(Command)]
#[command(result = "()", error = "MyError")]
pub struct DoSomething {
    pub id: u32,
}

#[derive(DomainEvent)]
#[domain_event(name = "SomethingHappened")]
pub struct SomethingHappened {
    pub id: u32,
}
```

See the crate documentation (`cargo doc --open`) for detailed usage.

---

Licensed under Apache-2.0. 