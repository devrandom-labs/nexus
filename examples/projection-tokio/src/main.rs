// examples/projection-tokio — a consumer-owned projection loop over
// nexus-store primitives (Projector, PersistTrigger, Subscription,
// SnapshotStore). nexus ships no event-loop runner; this demonstrates
// one concrete loop under tokio. See src/lib.rs for the loop itself.
//
// Run with: cargo run -p nexus-example-projection-tokio

#![allow(
    clippy::print_stdout,
    reason = "example code prints to demonstrate output"
)]

fn main() {
    println!(
        "Projection is a consumer-owned loop over nexus-store primitives. \
         See `cargo test -p nexus-example-projection-tokio` and src/lib.rs."
    );
}
