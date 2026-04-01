# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-macros-v0.1.0) - 2026-04-01

### Added

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

- pass nix flake check — elite clippy, audit ignore, trybuild refresh
- *(macros)* removed enum support
- *(macros)* fixed macro errors
- *(macros)* updated domain name macros
- *(macros)* fixed paths after macro addition

### Other

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
