//! Runnable narration of the three end-to-end phases. The real, gate-checked
//! proofs are the `#[tokio::test]`s in `lib.rs`; this binary just drives the
//! same three functions for a human and prints what each one observed.
//!
//! Run with: `cargo run -p nexus-example-fjall-end-to-end`

#![allow(clippy::print_stdout, reason = "example narrates progress to stdout")]

use nexus_example_fjall_end_to_end::{
    run_export_import, run_persistence, run_produce_and_sync, run_subscription,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir = tempfile::tempdir()?;

    println!("=== Phase 1: persistence lifecycle (write → close → reopen → rehydrate) ===");
    let (before, after) = run_persistence(&dir.path().join("persist")).await?;
    println!("  before close: {before:?}");
    println!("  after reopen: {after:?}");
    println!("  state survived the round-trip: {}", before == after);

    println!("\n=== Phase 2: subscription (catch-up → live tail) ===");
    let sub = run_subscription(&dir.path().join("subscribe")).await?;
    println!(
        "  catch-up versions {:?} → balance {}",
        sub.catchup_versions, sub.catchup_balance
    );
    println!(
        "  observed a LIVE append: v{} → balance {}",
        sub.live_version, sub.live_balance
    );
    println!(
        "  strict-after resume from v3 began at v{} (no redelivery)",
        sub.resumed_first_version
    );

    println!("\n=== Phase 3: export → CBOR box → file → import (round-trip) ===");
    let round_trip = run_export_import(&dir.path().join("backup")).await?;
    for (original, restored) in round_trip.originals.iter().zip(&round_trip.restored) {
        println!(
            "  {}: balance {} → restored {} ({})",
            original.id,
            original.state.balance,
            restored.state.balance,
            if original == restored {
                "identical"
            } else {
                "DIFFERS!"
            },
        );
    }
    println!(
        "  corruption surfaced as {:?}; malformed framing detected: {}",
        round_trip.corrupt_outcome, round_trip.malformed_detected
    );

    println!("\n=== Phase 4: IoT produce-and-sync (AllIndex::Disabled) ===");
    let iot = run_produce_and_sync(&dir.path().join("iot")).await?;
    println!(
        "  appended 3 readings, per-stream rehydrated balance {} (produce path unaffected)",
        iot.rehydrated_balance
    );
    println!(
        "  read_all → AllIndexDisabled: {} (no $all index maintained → smaller on-disk store)",
        iot.all_index_disabled
    );

    println!(
        "\nDone — fjall persistence + live subscription + export/import + \
         produce-and-sync all demonstrated."
    );
    Ok(())
}
