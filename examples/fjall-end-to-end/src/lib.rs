//! End-to-end demonstration of the **complete persistent path** on the real
//! `nexus-fjall` adapter — the three about-to-freeze surfaces that previously
//! had zero example coverage, composed over one domain:
//!
//! 1. **Persistence lifecycle** ([`run_persistence`]) — persist an aggregate to
//!    a real on-disk `FjallStore` via the typed repository, close the keyspace,
//!    reopen it, and rehydrate: the state survives the round-trip.
//! 2. **Subscription** ([`run_subscription`]) — open the catch-up + live-tail
//!    cursor, drain existing history, then observe a *genuinely live* append on
//!    the same stream; plus strict-after resume (a reopened cursor does not
//!    redeliver).
//! 3. **Export / import** ([`run_export_import`]) — back several streams up
//!    through the CBOR box to a file on disk, restore into a fresh store, and
//!    assert the restored aggregates rehydrate **identical** to the originals;
//!    plus the `Corrupt` (per-block crc) vs `Malformed` (framing) distinction.
//! 4. **Produce-and-sync `IoT`** ([`run_produce_and_sync`]) — build a device store
//!    with `AllIndex::Disabled`: append + per-stream rehydrate work as before,
//!    the `$all` (`events_global`) frame copy is skipped (smaller on-disk store),
//!    and `read_all` reports `AllIndexDisabled` explicitly (#270).
//!
//! The proofs live in `#[tokio::test]`s (run by the gate's nextest); `main.rs`
//! drives the same three functions for a human. Nothing here touches a
//! production crate — it is a consumer of the frozen public API, which doubles
//! as a last-chance ergonomics review of those surfaces.
//!
//! ## Aggregate binding (#243)
//!
//! The aggregate is named **once**, at `store.repository::<BankAccount>()`.
//! The resulting facade implements `Repository<BankAccount>` for exactly that
//! aggregate, so `repo.load(id)` / `repo.save(..)` infer it with no per-call
//! annotation. (This example was the canary for #243; with that landed, the
//! former `let acct: AggregateRoot<BankAccount> = repo.load(id)…` annotations
//! are gone.)

// Example code relaxes a handful of strict lints locally (production crates do
// NOT) — same posture as `examples/closing-the-books` and `examples/inmemory`.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "example: error/panic conditions are obvious from the narrative"
)]
#![allow(
    clippy::too_long_first_doc_paragraph,
    reason = "example: narrative doc comments lead with a full sentence"
)]
#![allow(
    clippy::expect_used,
    reason = "example: expect documents an assumption inside the spawned writer"
)]
#![allow(
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    reason = "example: shadowing `decided`/`repo` reads clearly here"
)]
#![allow(
    clippy::assigning_clones,
    reason = "example: clarity over the clone_into micro-optimization"
)]
#![allow(
    clippy::result_large_err,
    reason = "example surfaces the adapter's own (large) error type directly"
)]

pub mod domain;

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use futures::TryStreamExt;
use nexus::Version;
use nexus_fjall::{AllIndex, FjallStore};
use nexus_store::cbor::{ChunkWriter, decode_chunk};
use nexus_store::export::{EventExporter, StreamLister};
use nexus_store::import::{Atomicity, EventImporter, StreamOutcome};
use nexus_store::repository::Repository;
use nexus_store::store::{RawEventStore, Store};
use nexus_store::{PersistedEnvelope, StreamKey, Subscription};

use domain::{AccountEvent, AccountId, AccountState, BankAccount, Deposit, OpenAccount, Withdraw};

/// One boxed error domain for the example (every underlying error —
/// `FjallError`, the repository error, `ChunkError`, `ImportError`, `io::Error`
/// — is `Error + Send + Sync`, so `?` folds them all into this).
type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// Open an account and apply a series of deposits via the typed repository,
/// returning the resulting in-memory state. Generic over any `Repository` for
/// the account aggregate, so the bare `EventStore` facade and the snapshotting
/// decorator both satisfy the bound.
async fn seed_account<R: Repository<BankAccount>>(
    repo: &R,
    id: &AccountId,
    owner: &str,
    deposits: &[u64],
) -> Result<AccountState, BoxErr> {
    let mut account = repo.load(id.clone()).await?;
    let decided = account.handle(OpenAccount {
        owner: owner.to_owned(),
    })?;
    repo.save(&mut account, &decided).await?;
    for &amount in deposits {
        let decided = account.handle(Deposit { amount })?;
        repo.save(&mut account, &decided).await?;
    }
    Ok(account.state().clone())
}

/// Fold a raw subscription envelope's payload into a running balance — the
/// "consumer holds the codec" model: `Subscription` yields raw
/// [`PersistedEnvelope`]s, the consumer decodes them.
fn fold_balance(balance: u64, env: &PersistedEnvelope) -> Result<u64, BoxErr> {
    let event: AccountEvent = serde_json::from_slice(env.payload())?;
    Ok(match event {
        AccountEvent::Opened(_) => balance,
        AccountEvent::Deposited(d) => balance + d.amount,
        AccountEvent::Withdrawn(w) => balance - w.amount,
    })
}

// ════════════════════════════════════════════════════════════════════════════
// Phase 1 — persistence lifecycle: write → close → reopen → rehydrate
// ════════════════════════════════════════════════════════════════════════════

/// Persist an account, **close** the keyspace, **reopen** it, and rehydrate.
/// Returns `(before_close, after_reopen)` states; they must be equal.
pub async fn run_persistence(path: &Path) -> Result<(AccountState, AccountState), BoxErr> {
    let id = AccountId("alice".to_owned());

    // Scope the first store so it (and its Arc) drops at block end, closing and
    // flushing the keyspace before we reopen the same path.
    let before = {
        let store = FjallStore::builder(path).open()?.into_store();
        let repo = store.repository::<BankAccount>().build();
        seed_account(&repo, &id, "Alice", &[1_000, 500]).await?;
        // One more command so the persisted stream isn't trivial. `repo` is bound
        // to `BankAccount` (named at `repository::<BankAccount>()`), so `load`
        // infers the aggregate — no annotation (#243).
        let mut account = repo.load(id.clone()).await?;
        let decided = account.handle(Withdraw { amount: 300 })?;
        repo.save(&mut account, &decided).await?;
        account.state().clone()
    };

    // Reopen from scratch and rehydrate purely from the on-disk event log.
    let store = FjallStore::builder(path).open()?.into_store();
    let repo = store.repository::<BankAccount>().build();
    let reopened = repo.load(id).await?;
    let after = reopened.state().clone();

    Ok((before, after))
}

// ════════════════════════════════════════════════════════════════════════════
// Phase 2 — subscription: catch-up then a genuinely live tail
// ════════════════════════════════════════════════════════════════════════════

/// What the subscription phase observed, for the test to assert on.
#[derive(Debug)]
pub struct SubscriptionOutcome {
    /// Versions seen during catch-up over existing history.
    pub catchup_versions: Vec<u64>,
    /// Read-model balance after folding the catch-up prefix.
    pub catchup_balance: u64,
    /// Version of the event observed *live* on the tail (appended after
    /// catch-up drained).
    pub live_version: u64,
    /// Read-model balance after folding the live event.
    pub live_balance: u64,
    /// First version a fresh `subscribe(_, Some(v3))` delivers — must be 4, not
    /// 1 (strict-after resume: no redelivery).
    pub resumed_first_version: u64,
}

/// Open a subscription, drain catch-up history, observe a live append, and
/// demonstrate strict-after resume.
pub async fn run_subscription(path: &Path) -> Result<SubscriptionOutcome, BoxErr> {
    let store = FjallStore::builder(path).open()?.into_store();
    let repo = store.repository::<BankAccount>().build();
    let id = AccountId("alice".to_owned());

    // Seed exactly three events (open + 2 deposits → v1, v2, v3).
    seed_account(&repo, &id, "Alice", &[1_000, 500]).await?;

    let subscription = Subscription::new(&store);

    // Catch-up: the cursor never returns `None`, and we seeded a known count,
    // so drain exactly three. The stream is `!Unpin` → pin before polling.
    let stream = subscription.subscribe(&id, None)?;
    tokio::pin!(stream);
    let mut catchup_versions = Vec::new();
    let mut balance = 0u64;
    for _ in 0..3 {
        let env = stream
            .next()
            .await
            .ok_or("subscription closed during catch-up")??;
        catchup_versions.push(env.version().as_u64());
        balance = fold_balance(balance, &env)?;
    }
    let catchup_balance = balance;

    // Live tail: a separate task appends a NEW event (v4) after a barrier
    // rendezvous; the same parked cursor must observe it.
    let writer_store = store.clone();
    let writer_id = id.clone();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let writer_barrier = std::sync::Arc::clone(&barrier);
    let writer = tokio::spawn(async move {
        let repo = writer_store.repository::<BankAccount>().build();
        writer_barrier.wait().await;
        let mut account = repo.load(writer_id).await.expect("load for live append");
        let decided = account
            .handle(Deposit { amount: 250 })
            .expect("live deposit decides");
        repo.save(&mut account, &decided)
            .await
            .expect("live append persists");
    });

    barrier.wait().await;
    // Bound the live observation with a timeout — the cursor would otherwise
    // park forever waiting for the next event.
    let env = timeout_next(&mut stream, "live tail").await?;
    let live_version = env.version().as_u64();
    let live_balance = fold_balance(balance, &env)?;
    writer
        .await
        .map_err(|e| format!("writer task panicked: {e}"))?;

    // Strict-after resume: a fresh cursor from Some(v3) must begin at v4.
    let v3 = Version::new(3).ok_or("v3 is nonzero")?;
    let resume = subscription.subscribe(&id, Some(v3))?;
    tokio::pin!(resume);
    let first = timeout_next(&mut resume, "resume").await?;
    let resumed_first_version = first.version().as_u64();

    Ok(SubscriptionOutcome {
        catchup_versions,
        catchup_balance,
        live_version,
        live_balance,
        resumed_first_version,
    })
}

/// Poll the next item from a never-`None` subscription stream, bounded by a
/// timeout so a stuck tail fails the example rather than hanging it.
async fn timeout_next<S>(stream: &mut S, what: &str) -> Result<PersistedEnvelope, BoxErr>
where
    S: futures::Stream<Item = Result<PersistedEnvelope, nexus_fjall::FjallError>> + Unpin,
{
    let item = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .map_err(|_| format!("timed out waiting for {what} event"))?
        .ok_or_else(|| format!("subscription closed during {what}"))?;
    Ok(item?)
}

// ════════════════════════════════════════════════════════════════════════════
// Phase 3 — export → CBOR box → file → import, round-trip aggregate equality
// ════════════════════════════════════════════════════════════════════════════

/// One account's id + final state, for round-trip comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSummary {
    pub id: String,
    pub state: AccountState,
}

/// What the export/import phase produced.
#[derive(Debug)]
pub struct RoundTripOutcome {
    /// Source aggregates (id + final state), in id order.
    pub originals: Vec<AccountSummary>,
    /// Restored aggregates after backup → restore, in the same order.
    pub restored: Vec<AccountSummary>,
    /// Per-stream outcome of importing a chunk with one corrupted block.
    pub corrupt_outcome: StreamOutcome,
    /// Whether unparseable framing surfaced as `ChunkError::Malformed`.
    pub malformed_detected: bool,
}

/// Back up every stream through the CBOR box, driving `ChunkWriter` from the
/// export streams with `TryStreamExt::try_fold` — one functional pipeline, no
/// manual byte concatenation, no hand-ordered framing (#246).
async fn build_chunk(store: &Store<FjallStore>) -> Result<Vec<u8>, BoxErr> {
    let mut ids: Vec<StreamKey> = store
        .list_streams()
        .await?
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
    ids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let writer = futures::stream::iter(ids.into_iter().map(Ok::<_, BoxErr>))
        .try_fold(
            ChunkWriter::new(Vec::new(), None)?,
            |mut w, id| async move {
                let events = store.export_stream(&id, Version::INITIAL).await?;
                w.section(id.as_bytes())?.try_extend(events).await?;
                Ok(w)
            },
        )
        .await?;
    Ok(writer.into_sink())
}

/// Populate several streams, back them up to a file, restore into a fresh
/// store, and prove the restored aggregates rehydrate identical to the
/// originals. Also exercises the `Corrupt` vs `Malformed` distinction.
///
/// `work_dir` holds the three keyspaces (`src`, `dst`, `probe`) and the backup
/// file (`backup.nxch`).
pub async fn run_export_import(work_dir: &Path) -> Result<RoundTripOutcome, BoxErr> {
    // 1. Populate several streams in the source store.
    let src = FjallStore::builder(work_dir.join("src"))
        .open()?
        .into_store();
    let src_repo = src.repository::<BankAccount>().build();
    let specs: [(&str, &str, &[u64]); 3] = [
        ("alice", "Alice", &[1_000, 500]),
        ("bob", "Bob", &[200]),
        ("carol", "Carol", &[50, 50, 50]),
    ];
    let mut originals = Vec::new();
    for (id, owner, deposits) in specs {
        let account_id = AccountId(id.to_owned());
        let state = seed_account(&src_repo, &account_id, owner, deposits).await?;
        originals.push(AccountSummary {
            id: id.to_owned(),
            state,
        });
    }

    // 2. Export every stream through the CBOR box to a real file on disk.
    let chunk = build_chunk(&src).await?;
    let chunk_path = work_dir.join("backup.nxch");
    std::fs::write(&chunk_path, &chunk)?;

    // 3. Reopen the file, decode it, and import into a FRESH store.
    let sections = decode_chunk(&std::fs::read(&chunk_path)?)?;
    let dst = FjallStore::builder(work_dir.join("dst"))
        .open()?
        .into_store();
    dst.import(&sections, StreamKey::from_slice, Atomicity::WholeChunk)
        .await?;

    // 4. Rehydrate each aggregate from the destination and compare to source.
    let dst_repo = dst.repository::<BankAccount>().build();
    let mut restored = Vec::new();
    for summary in &originals {
        let restored_root = dst_repo.load(AccountId(summary.id.clone())).await?;
        let state = restored_root.state().clone();
        restored.push(AccountSummary {
            id: summary.id.clone(),
            state,
        });
    }

    // 5a. Corrupt one block's body (flip the last byte → its crc no longer
    //     matches). Framing still parses, so the bad block decodes to
    //     `ImportBlock::Corrupt`; a per-stream import halts that stream's good
    //     prefix → `StreamOutcome::Corrupt`.
    let mut corrupted = chunk;
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xFF;
    let corrupt_sections = decode_chunk(&corrupted)?;
    let probe = FjallStore::builder(work_dir.join("probe"))
        .open()?
        .into_store();
    let report = probe
        .import(
            &corrupt_sections,
            StreamKey::from_slice,
            Atomicity::PerStream,
        )
        .await?;
    let corrupt_outcome = report
        .streams()
        .iter()
        .map(|s| s.outcome)
        .find(|o| !o.is_complete())
        .ok_or("expected one corrupted stream in the report")?;

    // 5b. Unparseable framing is a *decode* error, a distinct failure domain.
    let malformed_detected = matches!(
        decode_chunk(b"not a valid nxch chunk"),
        Err(nexus_store::cbor::ChunkError::Malformed(_))
    );

    Ok(RoundTripOutcome {
        originals,
        restored,
        corrupt_outcome,
        malformed_detected,
    })
}

// ════════════════════════════════════════════════════════════════════════════
// Phase 4 — produce-and-sync IoT device: AllIndex::Disabled (#270)
// ════════════════════════════════════════════════════════════════════════════

/// What the produce-and-sync phase observed.
#[derive(Debug)]
pub struct ProduceSyncOutcome {
    /// Balance rehydrated from the on-disk per-stream log — the produce +
    /// per-stream-read path is unaffected by disabling `$all`.
    pub rehydrated_balance: u64,
    /// `true` if `read_all` reported the `$all` index disabled — proof the
    /// device maintains no `events_global` copy.
    pub all_index_disabled: bool,
}

/// A produce-and-sync `IoT` device: it appends per-stream and would export/sync
/// upward, but never reads `$all` locally. Built with
/// [`AllIndex::Disabled`](nexus_fjall::AllIndex::Disabled), it **skips the
/// `events_global` frame copy** on every append (a smaller on-disk store, less
/// flash write) — and `read_all` says so explicitly rather than silently
/// returning nothing. Append + per-stream rehydrate still work exactly as before.
pub async fn run_produce_and_sync(path: &Path) -> Result<ProduceSyncOutcome, BoxErr> {
    let store = FjallStore::builder(path)
        .all_index(AllIndex::Disabled)
        .open()?
        .into_store();
    let id = AccountId("device-1".to_owned());

    // The produce path is unaffected: append + per-stream rehydrate both work.
    let repo = store.repository::<BankAccount>().build();
    seed_account(&repo, &id, "Device", &[10, 20, 30]).await?;
    let rehydrated_balance = repo.load(id).await?.state().balance;

    // The `$all` index is not maintained — `read_all` surfaces that explicitly.
    let all_index_disabled = matches!(
        store.read_all(None).await,
        Err(nexus_fjall::FjallError::AllIndexDisabled)
    );

    Ok(ProduceSyncOutcome {
        rehydrated_balance,
        all_index_disabled,
    })
}

// ════════════════════════════════════════════════════════════════════════════
// Tests — the gate's nextest runs these (the real proofs).
// ════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persistence_survives_close_and_reopen() -> Result<(), BoxErr> {
        let dir = tempfile::tempdir()?;
        let (before, after) = run_persistence(&dir.path().join("db")).await?;

        // 1000 + 500 - 300 = 1200, and the rehydrated state is identical.
        assert_eq!(before.balance, 1_200);
        assert_eq!(before.owner, "Alice");
        assert!(before.is_open);
        assert_eq!(
            before, after,
            "rehydrated state must equal the pre-close state",
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscription_catches_up_then_tails_a_live_append() -> Result<(), BoxErr> {
        let dir = tempfile::tempdir()?;
        let outcome = run_subscription(&dir.path().join("db")).await?;

        // Catch-up replayed exactly the seeded history.
        assert_eq!(outcome.catchup_versions, vec![1, 2, 3]);
        assert_eq!(outcome.catchup_balance, 1_500);
        // The live append (v4, +250) was observed on the tail, not in catch-up.
        assert_eq!(outcome.live_version, 4);
        assert_eq!(outcome.live_balance, 1_750);
        // Strict-after resume from v3 begins at v4 — no redelivery of 1..3.
        assert_eq!(outcome.resumed_first_version, 4);
        Ok(())
    }

    #[tokio::test]
    async fn export_import_round_trip_rehydrates_identical_aggregates() -> Result<(), BoxErr> {
        let dir = tempfile::tempdir()?;
        let outcome = run_export_import(dir.path()).await?;

        // Round-trip aggregate equality: restored == originals, in order.
        assert_eq!(
            outcome.restored, outcome.originals,
            "restored aggregates must rehydrate identical to the originals",
        );
        // Three distinct accounts actually made the trip.
        assert_eq!(outcome.originals.len(), 3);
        assert_eq!(outcome.originals[0].state.balance, 1_500); // alice
        assert_eq!(outcome.originals[1].state.balance, 200); // bob
        assert_eq!(outcome.originals[2].state.balance, 150); // carol

        // Corruption distinctions: a crc-failed block halts its stream (Corrupt,
        // carrying the good prefix), while bad framing is Malformed.
        assert!(
            matches!(outcome.corrupt_outcome, StreamOutcome::Corrupt { .. }),
            "a crc-failed block must surface as StreamOutcome::Corrupt, got {:?}",
            outcome.corrupt_outcome,
        );
        assert!(
            outcome.malformed_detected,
            "unparseable framing must surface as ChunkError::Malformed",
        );
        Ok(())
    }

    #[tokio::test]
    async fn produce_and_sync_disables_the_all_index() -> Result<(), BoxErr> {
        let dir = tempfile::tempdir()?;
        let outcome = run_produce_and_sync(&dir.path().join("db")).await?;

        // The produce + per-stream-read path is unaffected: 10 + 20 + 30 = 60.
        assert_eq!(outcome.rehydrated_balance, 60);
        // read_all reports the $all index disabled — no events_global copy kept.
        assert!(
            outcome.all_index_disabled,
            "read_all must report AllIndexDisabled on an AllIndex::Disabled store",
        );
        Ok(())
    }
}
