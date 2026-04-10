# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-macros-v0.1.0) - 2026-04-10

### Added

- [**breaking**] fjall adapter, store refactoring, serde codecs, repository builder ([#138](https://github.com/devrandom-labs/nexus/pull/138))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- add in-memory bank account example
- *(macros)* add #[nexus::aggregate] attribute macro
- flatten nexus crate — kernel becomes top-level, remove old modules
- *(macros)* rewrite DomainEvent derive to work on enums with per-variant name()
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(nexus)* added custom result type
- *(nexus)* added name field for domain event
- *(macros)* added domain event macro
- *(macros)* made find fields morerobust
- *(macros)* enforced variant chck
- *(macros)* added domain event macro
- *(macros)* added query macro
- *(rusqlite)* added command macro
- *(macros)* added get attribute fnc
- *(macros)* added message macro
- *(tixlys)* added nexus-macros crate
- *(tixlys)* added age-keygen sops

### Fixed

- *(ci)* regenerate workspace-hack + add pre-commit hook
- *(kernel)* M2 — replace Default with AggregateState::initial()
- *(macros)* M6 — preserve user attributes through aggregate macro
- *(macros)* H6 — redacted Debug output, no internal state leakage
- *(macros)* reject empty event enums at compile time instead of unreachable
- pass nix flake check — elite clippy, audit ignore, trybuild refresh
- *(macros)* removed enum support
- *(macros)* fixed macro errors
- *(macros)* updated domain name macros
- *(macros)* fixed paths after macro addition

### Other

- fix formatting from sed-inserted initial() methods
- *(macros)* add roundtrip expand-compile test
- *(macros)* add exhaustiveness regression test
- *(macros)* add 7 property-based tests for macro-generated aggregates
- *(macros)* add adversarial input tests + fix empty enum DomainEvent
- *(macros)* expand macro hygiene tests to 7 cases
- *(macros)* add cross-crate boundary tests
- *(macros)* add macro hygiene tests
- *(macros)* add 9 compile-failure tests for macro error messages
- add release-plz for automated crate publishing
- update README with kernel documentation and verification table
- update workspace dependencies to latest compatible versions
- *(docs)* updated readme
- *(tixlys)* fixing clippy warnings
- *(macros)* clippy warning applied
- *(nexus)* updated test cases and macros
- *(nexus)* fixing tests
- *(nexus)* removed id from domain events
- *(nexus)* updated integrations tests
- *(rusqlite)* added test for append
- *(macors)* fixed clippy errors
- *(macros)* added aggregate type macro
- *(macros)* added field type
- *(macros)* added domain_event macro
- *(macro)* added command file
- *(macros)* added parse_command
- *(macros)* added skeleton for other core types
- *(macro)* added query one
- *(macros)* added main for macro
- *(macros)* simple derive macro
- *(macros)* added macros skeleton
- *(rusqlite)* added embedded migrations
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
