#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::as_conversions,
    clippy::str_to_string,
    reason = "test harness — relaxed lints for test code"
)]

#[path = "kernel_tests/aggregate_root_tests.rs"]
mod aggregate_root_tests;
#[path = "kernel_tests/architecture_tests.rs"]
mod architecture_tests;
#[path = "kernel_tests/edge_case_tests.rs"]
mod edge_case_tests;
#[path = "kernel_tests/error_tests.rs"]
mod error_tests;
#[path = "kernel_tests/events_tests.rs"]
mod events_tests;
#[path = "kernel_tests/integration_test.rs"]
mod integration_test;
#[path = "kernel_tests/property_tests.rs"]
mod property_tests;
#[path = "kernel_tests/static_assertion_tests.rs"]
mod static_assertion_tests;
#[path = "kernel_tests/traits_tests.rs"]
mod traits_tests;
#[path = "kernel_tests/version_tests.rs"]
mod version_tests;
