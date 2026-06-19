# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-store-testing-v0.1.0) - 2026-06-19

### Added

- *(store)* projection core traits + runner design (#155, #157) ([#161](https://github.com/devrandom-labs/nexus/pull/161))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(tixlys)* added age-keygen sops

### Other

- [**breaking**] retire nexus-framework; projection is primitives, the loop is the consumer's ([#193](https://github.com/devrandom-labs/nexus/pull/193))
- PR3 of bytes-envelope refactor — README, rustdoc, CLAUDE.md ([#184](https://github.com/devrandom-labs/nexus/pull/184))
- *(store, fjall, framework)* [**breaking**] stream + codec collapse + wire alignment — PR2 of bytes-envelope refactor ([#183](https://github.com/devrandom-labs/nexus/pull/183))
- *(store,fjall)* [**breaking**] Bytes-based envelopes with Range<u32> offsets — PR1 of bytes-envelope refactor ([#182](https://github.com/devrandom-labs/nexus/pull/182))
- *(store)* generalize EventStream with Item<'a> GAT + lending combinators (Map/TryMap/TryScan) ([#170](https://github.com/devrandom-labs/nexus/pull/170))
- enforce clippy across the whole workspace ([#167](https://github.com/devrandom-labs/nexus/pull/167))
- *(store)* exhaustive stream.rs coverage + reusable adapter conformance suite ([#165](https://github.com/devrandom-labs/nexus/pull/165))
- update README with kernel documentation and verification table
- *(docs)* updated readme
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
