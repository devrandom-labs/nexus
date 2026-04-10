# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-store-v0.1.0) - 2026-04-10

### Added

- aggregate snapshots for fast rehydration ([#123](https://github.com/devrandom-labs/nexus/pull/123)) ([#142](https://github.com/devrandom-labs/nexus/pull/142))
- [**breaking**] fjall adapter, store refactoring, serde codecs, repository builder ([#138](https://github.com/devrandom-labs/nexus/pull/138))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(tixlys)* added age-keygen sops

### Other

- update README with kernel documentation and verification table
- *(docs)* updated readme
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
