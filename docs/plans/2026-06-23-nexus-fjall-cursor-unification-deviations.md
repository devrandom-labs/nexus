# nexus-fjall cursor unification — Deviation Log

Where the implementation (branch `refactor/fjall-cursor-unification`, commits
`b1736c3..HEAD`) diverged from the plan
(`2026-06-23-nexus-fjall-cursor-unification-plan.md`) and design
(`2026-06-22-nexus-fjall-cursor-unification-design.md`). Each entry: what
changed, why, and the impact.

Clippy-lint-driven micro-changes are intentionally **not** logged — strict
clippy is the project default, not a divergence from intent.

---

## 1. `Catchup` resume API: `read_from` (inclusive) + `next_pos`, not `read_after` / `start`

- **Plan/design:** `read_after(pos)` (a scan from *after* `pos`) plus
  `start(from: Option<Position>) -> Position` to seed the loop.
- **Implemented:** `read_from(from)` opens a bounded scan from `from`
  **inclusive**, and a single transition `next_pos(Option<Position>) ->
  Option<Position>` is applied uniformly for the initial open *and* every
  reopen: `None` → the first position; `Some(v)` → strictly after `v`
  (`v.next()`); overflow at the maximum → `None`.
- **Why:** the design's inclusive-`read_after` + `position_of` loop produced a
  confirmed **duplicate-on-reopen** (the last-delivered event was re-read on
  refill) and had no overflow story (a resume position at `Version::MAX` /
  `GlobalSeq::MAX` had nowhere to go). Folding the resume rule into one
  `next_pos` makes strict-after resume the single source of truth, and
  overflow-as-`None` lets the live loop park instead of re-reading the maximum
  forever.
- **Impact:** strict-after subscription resume with no duplicate on reopen;
  overflow is representable (loop parks). Regression-pinned by
  `catchup.rs::reopen_does_not_redeliver_last_event` and the `*_next_pos_transition`
  tests. The store's `read_stream` / `read_all` `from` bounds stay **inclusive**;
  strict-after is the loop's concern, not the adapter's.

## 2. `Subscription::subscribe` is synchronous and fallible (`Result<impl Stream, WakeError>`)

- **Plan/design:** an infallible `impl Stream` from `subscribe`.
- **Implemented:** `subscribe` / `subscribe_all` are **sync** and return
  `Result<impl Stream<Item = Result<PersistedEnvelope, RawEventStore::Error>> +
  Send, <S as WakeSource>::Error>`.
- **Why:** wake-registration can fail (e.g. a subscriber-count overflow), and
  that failure domain is distinct from in-stream read errors (rule 3: one
  variant = one failure domain). Returning the register error eagerly keeps the
  two domains separate and avoids inventing a `SubError` union that would jam
  register + read failures into one type.
- **Impact:** callers handle a register error up front (`?`) and read errors
  in-band as `Err` stream items; no error-type union. Mechanically, callers
  now write `let cursor = sub.subscribe(..)?;`.

## 3. `WakeSource` gained `type Registration`; `StreamNotifiers` uses an `Arc::new_cyclic` `Weak<Self>` self-ref

- **Plan/design:** `WakeSource` with `register`/`wake`; `arm` on the returned
  registration.
- **Implemented:** `WakeSource` carries an associated `type Registration:
  WakeRegistration`, and `StreamNotifiers::new()` builds via `Arc::new_cyclic`
  so the struct holds a `Weak<Self>` back-reference.
- **Why:** the `register(&self, ...)` trait method's receiver is fixed at
  `&self`, but it must build a `SubscriptionGuard` (the drop-guard) that owns an
  `Arc<StreamNotifiers>` for the reap-at-zero back-channel. Without an
  `&Arc<Self>` receiver the only way to recover the owning `Arc` is to upgrade a
  stored `Weak<Self>`; the upgrade always succeeds while the caller holds a
  strong ref. An explicit `Registration` associated type is required so `arm`
  can be an RPITIT future (no `Box`).
- **Impact:** the drop-guard lifecycle is preserved through the trait method
  with zero boxing; the indirection is invisible to adapter authors (they return
  their own `Registration` type).

## 4. `$all` wake routing splits the watch generation from the legacy `Notify`

- **Plan/design:** a single wake path.
- **Implemented:** the inherent `StreamNotifiers::wake(stream)` bumps the
  per-stream generation + per-stream `Notify` **and** the `$all` *watch
  generation* (so a `WakeReg` armed on `$all` wakes), but does **not** touch the
  legacy `$all` `Notify` — that stays driven solely by `wake_all`.
- **Why:** the new `WakeReg`/`arm` path parks on the `watch` generation, while
  the legacy `$all` `Notify` path (and its existing test) must keep its old
  behaviour during the transition. Routing the new wake through the generation
  only satisfies both: the new `$all` registration wakes on any per-stream
  commit, and the legacy `Notify` semantics are unchanged.
- **Impact:** old and new `$all` wake paths coexist; both the legacy
  `Notify`/`wake_all` test and the new generation-based registration tests pass.
  The double-bump on the production path (`wake` then `wake_all` per append) is
  benign — generations are compared for inequality and spurious wakes are
  permitted.

## 5. A `Scan: Unpin` bound surfaced; kept method-local, not on the trait

- **Plan/design:** did not anticipate the `Unpin` requirement.
- **Implemented:** the generic `live` loop carries `where C::Scan: Unpin`, and
  `Subscription::subscribe`/`subscribe_all` carry
  `where <S as RawEventStore>::Stream: Unpin` / `AllStream: Unpin`. fjall's
  `ScanCursor<S>` impls `Stream` only `for S: ScanStrategy + Unpin`.
- **Why:** `StreamExt::next` requires `Unpin`, and the scan is held by-value
  across awaits inside the `unfold` state. Placing the bound on the methods
  rather than the `Catchup`/`WakeSource` trait avoids over-constraining the seam
  for adapters whose scan might not need it.
- **Impact:** no trait-level over-constraint; fjall satisfies it trivially
  (`fjall::Iter` is already `Unpin`).

## 6. `Subscription` + `Store::arc()` gated behind `#[cfg(feature = "subscription")]`

- **Plan/design:** did not specify a feature gate.
- **Implemented:** the whole subscription machinery (`subscription`, `wake`,
  `catchup`, `subscription_cursor`, `notify`) and `Store::arc()` are gated under
  a new `subscription` feature (pulls `tokio` + `foldhash` + `parking_lot`).
- **Why:** under default features `Store::arc()` and the loop have no caller, so
  ungated they would be dead code; the runtime dependencies (`tokio` etc.) also
  should not be unconditional for a no-runner kernel-adjacent crate.
- **Impact:** the subscription surface is opt-in; default builds stay lean and
  runtime-agnostic.

## 7. nexus-store's `BatchSize` + `InMemoryStore`'s batch knob were KEPT (only fjall's `batch_size` removed)

- **Plan/design:** remove the batch machinery.
- **Implemented:** only fjall's `FjallStoreBuilder::batch_size` knob and the
  `VecDeque`/batch refill cursors were removed. `nexus-store`'s `BatchSize`
  newtype (`batch.rs`) and `InMemoryStore`'s batch configuration are retained.
- **Why:** the removal targeted the *fjall* refill machinery that the single
  lazy `ScanCursor` made obsolete. `BatchSize` is a generic, validated
  (`1..=MAX_BATCH`) store-side primitive still used by the in-memory test
  adapter; removing it was out of scope and would delete a live invariant
  carrier.
- **Impact:** fjall loses a meaningless resident-row cap; the store-side
  `BatchSize` primitive and the in-memory adapter's behaviour are unchanged.

## 8. Encoding encapsulation (plan task 5.4) done by relocating white-box tests in-crate

- **Plan/design:** offered two options to fix the `pub mod encoding` rule-4
  leak while keeping the white-box codec tests compiling — including a
  `#[cfg(feature = "testing")]` re-export of the codec helpers.
- **Implemented (per a user decision):** the module is renamed `wire_key` and
  made a fully **private** `mod`; the white-box codec tests were moved *inside*
  the crate (e.g. into `scan.rs` / `wire_key.rs` `#[cfg(test)]` blocks) instead
  of being reached via a feature-gated re-export. The test-only
  `encode_event_value` was deleted; benches encode via
  `nexus_store::wire::encode_frame`.
- **Why:** a `testing`-gated re-export would still expose internal codecs as
  public API surface; making the module private and co-locating the tests fixes
  the rule-4 leak without any public exposure.
- **Impact:** no internal codec is reachable downstream; the rule-4 violation is
  fully closed.

---

## Behavior changes (intended, recorded for completeness)

- **Bounded fjall reads are now snapshot-consistent** (repeatable-read as of
  `open`) instead of seeing mid-scan appends — a property of the single lazy
  `ScanCursor`. A never-ending subscription therefore opens a fresh cursor per
  refill rather than holding one across the backlog (which would pin the GC
  watermark).
- **`batch_size` removed** from `FjallStoreBuilder` (see deviation 7).
- **Subscription resume is strict-after** (no duplicate on reopen) via
  `next_pos` (see deviation 1).
