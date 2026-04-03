# Contributing to Nexus

All contributions are welcome: bug reports, tests, docs, examples, and code.

## Ground rules

1. For new features or big refactors, **open an issue** first so we can discuss it.
2. Keep the public API backward-compatible whenever possible.
3. Make sure CI passes: `nix flake check` runs clippy, fmt, tests, and audits.
4. For non-trivial PRs, add or update tests and documentation.

## Development setup

Prerequisites: [Nix](https://nixos.org/) with flakes enabled.

```bash
git clone https://github.com/devrandom-labs/nexus.git
cd nexus
nix develop   # or: direnv allow

cargo test --all
cargo clippy --all-targets -- --deny warnings
```

## Running tests

```bash
# All tests
cargo test --all

# Single crate
cargo test -p nexus
cargo test -p nexus-store

# Single test by name
cargo test -p nexus -- replay_rejects_version_gap
```

Property-based tests (proptest) are included and may take longer on first run.

## Commit guidelines

- Follow conventional commits: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`
- Keep commits focused; avoid mixing unrelated changes.
- Describe **what** and **why**, not just the code diff.

## Release process

Releases are automated via [release-plz](https://release-plz.ieni.dev/). When PRs merge to `main`, release-plz creates a release PR with version bumps and changelogs. Merging the release PR publishes to crates.io and creates GitHub releases.

## License

By contributing you agree that your contributions will be licensed under MIT OR Apache-2.0, at your choice.
