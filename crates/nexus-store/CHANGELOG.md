# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-store-v0.1.0) - 2026-06-04

### Added

- *(store, fjall)* SharedSubscription (Arc-based, 'static cursor) — PR1 of arc-subscription refactor ([#177](https://github.com/devrandom-labs/nexus/pull/177))
- *(store)* futures::Stream bridge for owning-output lending streams ([#171](https://github.com/devrandom-labs/nexus/pull/171))
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

### Other

- PR3 of bytes-envelope refactor — README, rustdoc, CLAUDE.md ([#184](https://github.com/devrandom-labs/nexus/pull/184))
- *(store, fjall, framework)* [**breaking**] stream + codec collapse + wire alignment — PR2 of bytes-envelope refactor ([#183](https://github.com/devrandom-labs/nexus/pull/183))
- *(store,fjall)* [**breaking**] Bytes-based envelopes with Range<u32> offsets — PR1 of bytes-envelope refactor ([#182](https://github.com/devrandom-labs/nexus/pull/182))
- *(store)* explain OwnedEventStream as the on-ramp to futures::Stream — PR4 of arc-subscription refactor ([#180](https://github.com/devrandom-labs/nexus/pull/180))
- *(store, fjall, framework)* [**breaking**] delete borrowed Subscription; promote Arc-based shape — PR3 of arc-subscription refactor ([#179](https://github.com/devrandom-labs/nexus/pull/179))
- *(store, macros)* [**breaking**] delete Upcaster trait; drop U generic; load_with + save_with for with-upcaster ergonomics ([#175](https://github.com/devrandom-labs/nexus/pull/175))
- *(store)* [**breaking**] combinator-chain replay_from via map_err; Arc-own facade components; expose Store::raw ([#174](https://github.com/devrandom-labs/nexus/pull/174))
- *(store, macros)* [**breaking**] slim Upcaster trait; macro gap-coverage check; drop UpcastError ([#173](https://github.com/devrandom-labs/nexus/pull/173))
- *(store)* [**breaking**] delete decoder facade; load paths run inline upcast+decode ([#172](https://github.com/devrandom-labs/nexus/pull/172))
- *(store)* generalize EventStream with Item<'a> GAT + lending combinators (Map/TryMap/TryScan) ([#170](https://github.com/devrandom-labs/nexus/pull/170))
- *(store, framework)* split Codec into independent Encode/Decode/BorrowingDecode traits ([#169](https://github.com/devrandom-labs/nexus/pull/169))
- enforce clippy across the whole workspace ([#167](https://github.com/devrandom-labs/nexus/pull/167))
- *(store, framework)* interruptible async fold + collapsed projection runner ([#166](https://github.com/devrandom-labs/nexus/pull/166))
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
