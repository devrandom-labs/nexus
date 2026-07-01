# nexus-postgres — contract-validation findings (#213)

**What this is:** the freeze-relevant findings from building a *second* store adapter (`nexus-postgres`, on `sqlx`-postgres) against the adapter-facing contract, before the 1.0 freeze. #213's thesis: a contract exercised by only one embedded adapter (fjall) has unstated single-process assumptions baked in; a second, networked, concurrent backend is the only thing that proves the boundary is drawn right. Feed these into #204–#210.

**Bottom line:** the post-#266 contract holds up. The `$all` position-seam redesign (adapter-defined `AllPosition`, position-tagged stream, `global_seq`-free frame) was exactly what a concurrent SQL adapter needed — postgres's live `$all` is now gap-free *by construction*, not by luck. The remaining findings are (a) API-surface drift the audit caught, (b) confirmation that the intentional inclusive/exclusive asymmetry is right, and (c) operational caveats to document, not contract bugs.

---

## 1. The #266 seam is validated — `$all` is gap-free by construction

The pre-#266 contract carried `GlobalSeq` (a single monotonic scalar) as *the* `$all` resume position. That was an **unstated fjall-ism**: fjall stamps the counter inside its single serialized write-tx, so insert-order == commit-order for free. Postgres cannot honor that — `BIGINT ... IDENTITY` is assigned at INSERT but rows are visible at COMMIT, and commit order ≠ insert order, so a late-committing lower scalar would be silently skipped by a `WHERE seq > bookmark` cursor (event loss).

#266 replaced the scalar with an **adapter-defined `AllPosition`** carried *alongside* each `$all` event (not derived from it), resumed by `Ord` (exclusive). Postgres defines `PgAllPos { txid, seq }` and reads with the Way-2 query: `WHERE txid < pg_snapshot_xmin(pg_current_snapshot()) AND (txid, seq) > $from ORDER BY txid, seq`. Two guards, both load-bearing:
- **watermark** (visibility): never deliver a row until every transaction id ≤ its `txid` has settled;
- **composite `Ord` resume** (checkpointing): strictly-after on `(txid, seq)`, which has no `+1` successor — exactly why the seam dropped the successor model for exclusive `Ord`.

**Evidence:** `tests/all_noskip_tests.rs` reproduces the exact interleaving the old scalar lost — txn A inserts (lower `global_seq`) and holds; txn B inserts (higher) and commits; `read_all` delivers **nothing** while A is in-flight (watermark), then **both** in `(txid, seq)` order with A present once A commits. Under the old contract this is the skip; under #266 it is correct by construction. This turns the adapter plan's original "demonstrate-the-skip-then-defer-a-fix" into a real no-skip linearizability proof.

**For the freeze (#204–#210):** the `AllPosition` trait + position-tagged `AllStream` + exclusive-`Ord` `read_all` are the right shape to freeze. Two adapters now implement them (fjall `GlobalSeq`, postgres `PgAllPos`), which was the point.

## 2. The inclusive-`read_stream` / exclusive-`read_all` asymmetry is correct — no friction

`read_stream(from: Version)` is **inclusive**; `read_all(from: Option<AllPosition>)` is **exclusive**. This looked like a smell worth watching. Building postgres confirmed it is *intentional and necessary*: a single stream has a gapless `Version` successor (so inclusive + the loop's `next_pos` works), but a composite `$all` position has none, so `$all` must resume by `Ord`-strictly-after. Postgres implements both cleanly (per-stream `version >= $from`; `$all` `(txid,seq) > $from`) with no contortion. **Keep the asymmetry; it is documented on the trait (CLAUDE rule 4).**

## 3. API-surface drift caught by the pre-build audit (freeze-relevant)

A second adapter is a fresh consumer of the *public* API, so it surfaced surface that only fjall's familiarity had smoothed over. All were fixed in-plan before coding, but they are worth noting for the freeze because they are exactly the kind of thing a 1.0 consumer hits:
- **`ErrorId` lives in `nexus`, not `nexus_store`.** A store adapter naturally reaches for `nexus_store::ErrorId` (it's used pervasively in store error types) and finds nothing — it must import `nexus::ErrorId`. Consider a `nexus_store` re-export before freeze so adapters have one import root.
- **Value newtypes construct via `from_bytes(Bytes)`, not `::new`.** `EventType`/`Payload`/`Metadata` — the `new` intuition fails. Naming is frozen surface; `from_bytes` is fine but should be the *only* obvious constructor (it is).
- **`schema_version` is `NonZeroU32`** — an adapter mapping it to a DB column must pick a width and narrow (postgres `INTEGER`/`i32` needs a checked `try_from` on write). Not a bug, just a documented consequence of the type.
- **`Store<S>` is the subscription front door.** `Subscription::new` takes `&Store<S>`, not the raw adapter — an adapter's own tests must `.into_store()` first. Correct (the `Store` handle is the substrate), just non-obvious.

## 4. `read_stream`/`read_all` batching contract confirmed permissive — good

The trait says an adapter *may* chunk/paginate internally but is not required to. Postgres's validation skeleton `fetch_all`s the whole result into a `Vec` (materialize-all) and still satisfies the contract — the externally-observable behaviour (ascending order, terminate when exhausted) is unchanged. This confirms the contract does not smuggle in fjall's lazy-cursor as a requirement. **The trait doc is clean here.** (Production postgres wants a keyset-paginated cursor — a follow-up, not a contract change.)

## 5. Operational caveats to document (not contract bugs)

- **Way-2 watermark stall.** A single long-running transaction anywhere holds `pg_snapshot_xmin` back and stalls *all* `$all` consumers until it finishes. Inherent to reader-side ordering; documented on `read_all`. The high-throughput WAL/logical-decoding path is deferred to #265.
- **`PgListener` drops NOTIFYs across auto-reconnect.** sqlx's `PgListener` silently loses notifications sent while its connection was down. This is safe *only* because the adapter routes wake through `nexus_store::notify::StreamNotifiers`, whose arm-before-confirm-rescan discipline re-scans on every wake — so a dropped NOTIFY only *delays* a wake, never loses an event. This validates that the `WakeSource` contract correctly treats wake as a *hint*, not a delivery guarantee — a distributed adapter depends on that. **Confirm the `WakeSource` docs state "wake is a hint; correctness rests on the catch-up scan."**
- **`xid8` has no `bigint` cast.** Minor adapter-internal wart (`txid::text::bigint` round-trip); not contract-visible.

## 6. Scoping decisions (deferred, carded)

- **No TLS** in the validation skeleton (`sqlx` `runtime-tokio`): local/socket Postgres needs none; `rustls` pulls a `deny.toml`-rejected license, `native-tls` pulls `openssl-sys` (fights Nix reproducibility + IoT cross-compile). TLS-to-managed-Postgres → **#268**.
- **Deferred (not needed to validate the core contract):** `SnapshotStore`, `AtomicAppend`, `StreamLister`, export/import on postgres; connection-pool tuning; migrations; keyset-paginated cursors.

## 7. Open items (validated on Linux CI, not macOS)

The adapter's 20 integration tests + the shared `nexus-store-testing` conformance suites run only under the Linux-only `postgres-integration` nixosTest (they skip-pass without `DATABASE_URL`, keeping the darwin dev gate fast/dockerless). The nixosTest's `services.postgresql` auth/role wiring and the nextest-archive-in-VM invocation could not be verified from macOS and will be confirmed by the CI job on the PR — expect at most a small auth-wiring iteration there.

---

**Net:** the second adapter did its job — it proved the #266 seam, confirmed the inclusive/exclusive asymmetry, and surfaced only import-ergonomics and doc-precision items for the freeze, no contract redesign. That is the outcome #213 was looking for.
