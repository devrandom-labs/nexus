# Contributing to Nexus

First off, thanks for taking the time to contribute! **All kinds of contributions are welcome**: bug reports, tests, docs, examples and, of course, code.

## Ground rules

1. Be kind and follow our [Code of Conduct](CODE_OF_CONDUCT.md).
2. If you’re adding a new feature or a big refactor, **open an issue** first so we can discuss it.
3. Keep the public API backward-compatible whenever possible.
4. Make sure CI tests pass (`cargo test --all`) and that `cargo fmt --all --check` is happy.
5. For non-trivial PRs, add or update documentation and examples.

## Development setup

```bash
# clone the repo
$ git clone https://github.com/nexus/nexus-core
$ cd nexus-core

# enter the nix dev-shell (preferred)
$ nix develop

# or use plain cargo
$ cargo test --all
```

## Running tests

We use [`bacon`](https://github.com/Canop/bacon) to make test-driven development pleasant but a plain `cargo test --all` works just fine.

Some crates include property-based tests that can take longer to run; feel free to use `--lib`, `--test XYZ`, etc. to scope what you run locally.

## Commit guidelines

* Follow the conventional commit style (e.g. `feat:` / `fix:` / `docs:`).
* Keep commits focused; avoid mixing unrelated changes.
* Add **context** in commit messages – what and *why*, not just the code diff.

## Release process

Releases are cut from the `main` branch and use [cargo-release](https://github.com/crate-ci/cargo-release).

## License

By contributing you agree that your contributions will be licensed under the terms of the MIT or Apache-2.0 license, at your choice. 