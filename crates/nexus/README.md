# nexus

Event-sourcing kernel — aggregates, events, versioning, command handling. No `Box<dyn>`, no runtime downcasting, no `std` allocation.

See the [root README](../../README.md) for usage and examples.

## Verification

The kernel is tested with 9 verification techniques:

| Technique | What it proves |
|-----------|---------------|
| Unit tests + edge cases | Correct behavior |
| Property-based testing (proptest) | Algebraic properties hold for all random inputs |
| Compile-failure tests (trybuild) | Invalid code fails to compile |
| Static assertions | Send, Sync, size, trait bounds enforced at compile time |
| Miri | Zero undefined behavior under strict provenance |
| Mutation testing (cargo-mutants) | Every viable mutation caught |
| Benchmarks (criterion) | Performance regression detection |
| Doc tests | All examples compile and run |
| Architecture tests | Kernel imports nothing from outer layers |

## License

Licensed under your choice of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE).
