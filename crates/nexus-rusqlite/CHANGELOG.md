# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/devrandom-labs/nexus/releases/tag/nexus-rusqlite-v0.1.0) - 2026-04-01

### Added

- *(nexus)* added docs
- *(helpers)* added pending event generation
- *(rusqlite)* added sequence check for events
- *(nexus)* introduced boxed events
- *(nexus)* validation in pending event builder
- *(helpers)* added arbitrary sequence of events
- *(helpers)* added prop test strategy module
- *(nexus)* added fake nexusId
- *(nexus)* added custom result type
- *(nexus)* added sqlite constraint error
- *(nexus)* made id trait more stricter
- *(nexus)* added copy trait to nexus_id
- *(nexus)* added name field for domain event
- *(macros)* added message macro
- *(tixlys)* added cargo tarpaulin
- *(rusqlite)* added build script
- *(rusqlite)* stream adding is async
- *(rusqlite)* added read stream event fn
- *(nexus)* added event record response
- *(rusqlite)* storing records
- *(rusqlite)* added version check
- *(rusqlite)* added build script,run on change
- *(nexus-rusqlite)* in memory store
- *(tixlys)* created rusqlite and sql schema

### Fixed

- *(nexus)* fixed test cases and clippy problems
- *(rusqlite)* fixed test cases
- *(rusqlite)* fixed stream_name inclusion
- *(rusqlite)* fixed data integrity check
- *(rusqlite)* updated conflict error
- *(rusqlite)* fixed clippy errors
- *(rusqlite)* expected version check
- *(helper)* fixed tests to use nonzerou64
- *(rusqlite)* no op check in save stream
- *(rusqlte)* checking stream_id as uuid
- *(nexus)* fixed pending event builder error
- *(tixlys)* fixed schema migration for nix
- *(macros)* fixed paths after macro addition
- *(rusqlite)* fixed build.rs

### Other

- *(rusqlite)* added pending events
- *(rusqlite)* added benchmark tests
- *(nexus)* added example folder and bench
- *(rusqlite)* fixed conflict check test case
- *(rusqlite)* fixed sequence check test
- *(rusqlite)* added sequence test check
- *(rusqlite)* added sequence check unit test
- *(rusqlite)* conflict prop test
- *(helpers)* added size for arbitrary sequence
- *(rusqlite)* added empty append stream test
- *(rusqlite)* rusqlite prop test file
- *(rusqlite)* added golden path prop test
- *(tixlys)* removed upgrade -i
- *(rusqlite)* added test helpers in rusqlite
- *(tixlys)* fixing clippy warnings
- *(rusqlite)* fixed serialized domain event
- *(nexus)* removed aggregate type
- *(rusqlite)* added proptest to rusqlite
- *(rusqlite)* cleaned up the tests
- *(rusqlite)* added partialeq
- *(rusqlite)* added unit test for rusqlite
- *(rusqlite)* made the error inline
- *(rusqlite)* removed the test case
- *(rusqlite)* added unique constraint check
- *(nexus)* clippy fixes
- *(nexus)* identity module for identity shit
- *(rusqlite)* basic testing in rusqlite
- *(nexus)* checking read side
- *(tixlys)* added futures crate
- *(rusqlite)* changed store new constructor
- *(nexus)* changed build to serialize
- *(rusqlite)* added test for append
- *(rusqlite)* added event record in test
- *(macors)* fixed clippy errors
- *(macros)* added main for macro
- *(macros)* simple derive macro
- *(tixlys)* schemas in root
- *(macros)* added macros skeleton
- *(rusqlite)* added embedded migrations
- *(rusqlite)* testing strategy
- *(rusqlite)* peppered println
- *(rusqlite)* clippy warnings
- *(rusqlite)* one shot for write
- *(rusqlite)* removed inline from configure
- *(rusqlite)* added conversion between db
- *(rusqlite)* added instrumentation
- *(rusqlite)* removed select clause
- *(rusqlite)* added approp tracing header
- *(rusqlite)* added error messages
- *(rusqlite)* made the store clonable
