# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-postgres-v0.1.0) - 2026-07-02

### Added

- *(postgres)* nexus-postgres — second store adapter, contract validation before freeze ([#213](https://github.com/devrandom-labs/nexus/pull/213)) ([#269](https://github.com/devrandom-labs/nexus/pull/269))
- *(store)* projection core traits + runner design (#155, #157) ([#161](https://github.com/devrandom-labs/nexus/pull/161))
- *(store)* [**breaking**] event store with save atomicity, schema versioning, and structured errors ([#122](https://github.com/devrandom-labs/nexus/pull/122))
- *(nexus)* added docs
- *(nexus)* removed tixlys related stuff
- *(tixlys)* added age-keygen sops

### Other

- [**breaking**] retire nexus-framework; projection is primitives, the loop is the consumer's ([#193](https://github.com/devrandom-labs/nexus/pull/193))
- PR3 of bytes-envelope refactor — README, rustdoc, CLAUDE.md ([#184](https://github.com/devrandom-labs/nexus/pull/184))
- update README with kernel documentation and verification table
- *(docs)* updated readme
- *(tixlys)* removed the last section of readme
- *(auth)* added test for fallback and doc
- *(readme)* updates status badges
- *(readme)* updated the status badges
- added github status bar
- initial nix flake setup
