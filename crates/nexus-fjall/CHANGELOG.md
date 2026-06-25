# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-fjall-v0.1.0) - 2026-06-25

### Added

- *(store)* add frame-format-version byte to wire layout ([#205](https://github.com/devrandom-labs/nexus/pull/205)) ([#229](https://github.com/devrandom-labs/nexus/pull/229))
- global $all subscription cursor (GlobalSeq-ordered, all-streams) ([#214](https://github.com/devrandom-labs/nexus/pull/214)) ([#216](https://github.com/devrandom-labs/nexus/pull/216))
- bound subscription/read batch size ([#176](https://github.com/devrandom-labs/nexus/pull/176)) ([#192](https://github.com/devrandom-labs/nexus/pull/192))
- *(store, fjall)* [**breaking**] per-stream subscription wake registry ([#191](https://github.com/devrandom-labs/nexus/pull/191))
- *(store, fjall)* SharedSubscription (Arc-based, 'static cursor) — PR1 of arc-subscription refactor ([#177](https://github.com/devrandom-labs/nexus/pull/177))
- *(store, fjall, framework)* SnapshotStore + per-event GlobalSeq ([#168](https://github.com/devrandom-labs/nexus/pull/168))
- *(store)* unified state persistence and projection rebuild ([#158](https://github.com/devrandom-labs/nexus/pull/158)) ([#163](https://github.com/devrandom-labs/nexus/pull/163))
- *(store)* subscription-powered projection runner ([#157](https://github.com/devrandom-labs/nexus/pull/157)) ([#162](https://github.com/devrandom-labs/nexus/pull/162))
- *(store)* projection core traits + runner design (#155, #157) ([#161](https://github.com/devrandom-labs/nexus/pull/161))
- [**breaking**] subscriptions, allocation-free errors, EventStream API fix ([#150](https://github.com/devrandom-labs/nexus/pull/150))
- aggregate snapshots for fast rehydration ([#123](https://github.com/devrandom-labs/nexus/pull/123)) ([#142](https://github.com/devrandom-labs/nexus/pull/142))
- [**breaking**] fjall adapter, store refactoring, serde codecs, repository builder ([#138](https://github.com/devrandom-labs/nexus/pull/138))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(tixlys)* added age-keygen sops

### Fixed

- *(store, fjall)* close wire/envelope schema_version asymmetry; harden decode ([#186](https://github.com/devrandom-labs/nexus/pull/186))

### Other

- pin stable toolchain via rust-toolchain.toml, declare MSRV ([#204](https://github.com/devrandom-labs/nexus/pull/204)) ([#226](https://github.com/devrandom-labs/nexus/pull/226))
- extract subscription loop to nexus-store (zero-cost, no Box); unify fjall cursors ([#224](https://github.com/devrandom-labs/nexus/pull/224))
- [**breaking**] retire nexus-framework; projection is primitives, the loop is the consumer's ([#193](https://github.com/devrandom-labs/nexus/pull/193))
- *(store, fjall, framework)* [**breaking**] collapse Subscription trait pair to concrete struct ([#181](https://github.com/devrandom-labs/nexus/pull/181)) ([#190](https://github.com/devrandom-labs/nexus/pull/190))
- *(store, fjall)* [**breaking**] shrink wire.rs to pure framing; SchemaVersion + PersistedEnvelope value accessors (PR3 of 3) ([#189](https://github.com/devrandom-labs/nexus/pull/189))
- *(store)* [**breaking**] PendingEnvelope holds value newtypes; builder fallible (PR2 of 3) ([#188](https://github.com/devrandom-labs/nexus/pull/188))
- PR3 of bytes-envelope refactor — README, rustdoc, CLAUDE.md ([#184](https://github.com/devrandom-labs/nexus/pull/184))
- *(store, fjall, framework)* [**breaking**] stream + codec collapse + wire alignment — PR2 of bytes-envelope refactor ([#183](https://github.com/devrandom-labs/nexus/pull/183))
- *(store,fjall)* [**breaking**] Bytes-based envelopes with Range<u32> offsets — PR1 of bytes-envelope refactor ([#182](https://github.com/devrandom-labs/nexus/pull/182))
- *(store, fjall, framework)* [**breaking**] delete borrowed Subscription; promote Arc-based shape — PR3 of arc-subscription refactor ([#179](https://github.com/devrandom-labs/nexus/pull/179))
- *(store)* generalize EventStream with Item<'a> GAT + lending combinators (Map/TryMap/TryScan) ([#170](https://github.com/devrandom-labs/nexus/pull/170))
- *(store, framework)* split Codec into independent Encode/Decode/BorrowingDecode traits ([#169](https://github.com/devrandom-labs/nexus/pull/169))
- enforce clippy across the whole workspace ([#167](https://github.com/devrandom-labs/nexus/pull/167))
- *(store)* exhaustive stream.rs coverage + reusable adapter conformance suite ([#165](https://github.com/devrandom-labs/nexus/pull/165))
- apply Section 6 functional-first style across store and fjall ([#143](https://github.com/devrandom-labs/nexus/pull/143))
- update README with kernel documentation and verification table
- *(docs)* updated readme
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
