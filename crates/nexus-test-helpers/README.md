# nexus-test-helpers

Handy utilities that make testing code built with [`nexus`](https://crates.io/crates/nexus) a breeze.

Features include:

* Property-based generators for common Nexus types.
* Helper types for asserting equivalence between `PendingEvent` and `PersistedEvent`.
* A sample *user* domain used by the integration tests in this workspace.

Add it as a dev-dependency:

```toml
[dev-dependencies]
nexus-test-helpers = "0.1"
```

---

Licensed under Apache-2.0. 