# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-framework-v0.1.0) - 2026-05-26

### Added

- *(store, fjall, framework)* SnapshotStore + per-event GlobalSeq ([#168](https://github.com/devrandom-labs/nexus/pull/168))
- *(store)* unified state persistence and projection rebuild ([#158](https://github.com/devrandom-labs/nexus/pull/158)) ([#163](https://github.com/devrandom-labs/nexus/pull/163))
- *(store)* subscription-powered projection runner ([#157](https://github.com/devrandom-labs/nexus/pull/157)) ([#162](https://github.com/devrandom-labs/nexus/pull/162))
- *(store)* projection core traits + runner design (#155, #157) ([#161](https://github.com/devrandom-labs/nexus/pull/161))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(tixlys)* added age-keygen sops

### Other

- *(store)* generalize EventStream with Item<'a> GAT + lending combinators (Map/TryMap/TryScan) ([#170](https://github.com/devrandom-labs/nexus/pull/170))
- *(store, framework)* split Codec into independent Encode/Decode/BorrowingDecode traits ([#169](https://github.com/devrandom-labs/nexus/pull/169))
- *(store, framework)* interruptible async fold + collapsed projection runner ([#166](https://github.com/devrandom-labs/nexus/pull/166))
- *(store)* exhaustive stream.rs coverage + reusable adapter conformance suite ([#165](https://github.com/devrandom-labs/nexus/pull/165))
- update README with kernel documentation and verification table
- *(docs)* updated readme
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
