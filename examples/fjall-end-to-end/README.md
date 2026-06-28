# fjall-end-to-end

The executable proof that the freeze-ready public API **composes** on the real
persistent adapter. Every other example uses `InMemoryStore`; this one runs the
complete path on `nexus-fjall`, exercising three surfaces that previously had
zero example coverage — composed over one bank-account domain.

```bash
cargo run -p nexus-example-fjall-end-to-end     # narrate the three phases
cargo test -p nexus-example-fjall-end-to-end    # the gate-checked proofs
```

## What it proves

1. **Persistence lifecycle** (`run_persistence`) — persist an aggregate to an
   on-disk `FjallStore` via the typed repository, **close** the keyspace,
   **reopen** it, and rehydrate. The rehydrated state equals the pre-close
   state.
2. **Subscription** (`run_subscription`) — open the catch-up + live-tail cursor,
   drain existing history, then observe a **genuinely live** append on the same
   stream (spawned writer + `Barrier`, bounded by a timeout because the cursor
   never returns `None`). Plus **strict-after resume**: a cursor reopened from
   `Some(v3)` starts at v4, no redelivery.
3. **Export / import** (`run_export_import`) — back several streams up through
   the CBOR box to a **file on disk**, restore into a fresh store, and assert the
   restored aggregates rehydrate **identical** to the originals. Plus the
   `Corrupt` (per-block crc) vs `Malformed` (framing) distinction.

The real assertions live in `#[tokio::test]`s (run by the gate's nextest);
`main.rs` drives the same three functions for a human.

## Ergonomics note (pre-freeze)

`EventStore<S, C>` implements `Repository<A>` for *every* aggregate `A`, so
`repo.load(id)` can't infer `A` — the inline call sites name it
(`let acct: AggregateRoot<BankAccount> = repo.load(id).await?`). Tracked in
#243 (aggregate-bound repository); when that lands, those annotations come out.
