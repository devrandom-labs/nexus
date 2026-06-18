//! # Closing the Books — bounded streams instead of snapshots
//!
//! "Closing the Books" is a way to model a long-running process so the
//! aggregate's stream stays short — short enough that you never need a snapshot
//! to load it quickly. It is the **preferred** alternative to snapshots in
//! Nexus whenever the domain has a natural cycle.
//!
//! ## The idea
//!
//! The name comes from accounting. At the end of each period — a day, a month,
//! a cashier's shift — you total everything up, write a closing report, and
//! carry only the closing balance into the next period. You do not re-read a
//! year of ledger entries to know today's opening balance.
//!
//! Translate that to event sourcing:
//!
//! 1. Model the process as a **bounded stream** with an explicit start event
//!    and an explicit end event.
//! 2. When the period ends, append a **summary event** holding exactly the
//!    state the next period needs to start — the closing balance, the totals,
//!    nothing more.
//! 3. The next period is a **brand-new stream** (a new aggregate id) opened
//!    from that carried-forward summary.
//!
//! A cash register is the classic example. Instead of one never-ending
//! `CashRegister` stream, model each shift as its own `CashierShift`:
//! `ShiftOpened` (with the opening float) then `TransactionRegistered` events,
//! then `ShiftClosed` — the summary, carrying the declared tender, any
//! overage or shortage, and the final float. The next shift opens a new stream
//! whose opening float is the previous shift's final float.
//!
//! ## Why this beats a snapshot
//!
//! A summary event *looks* like a snapshot — both let you skip replaying old
//! history — but a summary event is a **first-class domain event**:
//!
//! - It flows through the same [`AggregateState::apply`](crate::AggregateState::apply)
//!   fold and the same upcaster pipeline as every other event, so it versions
//!   like any event. A snapshot is opaque bytes the kernel never interprets.
//! - In the store, a snapshot's schema version is a **cache key, not a
//!   migration engine**: change the state's shape, bump the version, and old
//!   snapshots are silently ignored — every aggregate falls back to a full
//!   replay until a fresh snapshot is written. There is no upcasting path for
//!   snapshot bytes. A summary event has none of this; you evolve it with the
//!   same `#[nexus::transforms]` upcasters you already use for events.
//! - It carries the *minimum* state the next period needs, chosen by the
//!   domain — not a dump of whatever happens to sit in the aggregate struct.
//!
//! So reach for `CashierShift` before you reach for the snapshot decorator
//! (`Snapshotting`, in the `nexus-store` crate).
//!
//! ## What Nexus gives you (and what it does not)
//!
//! Closing the Books is a **modeling discipline**, not an API. Nexus ships no
//! "close stream", "archive", or "delete" operation, and needs none:
//!
//! - Starting the next period is just constructing a fresh
//!   [`AggregateRoot`](crate::AggregateRoot) with a new id; the store creates
//!   the stream on first append.
//! - The summary is just a variant in your event enum, derived with
//!   [`DomainEvent`](crate::DomainEvent).
//!
//! See the runnable `examples/closing-the-books` crate for the full contrast:
//! it builds a long-lived register that would need a snapshot and a
//! shift-bounded register side by side, and prints how many events each must
//! replay to read the current float.
//!
//! ## When NOT to use it
//!
//! Short streams are a tool, not a rule. Keep the stream long (and snapshot if
//! you must) when:
//!
//! - **There is no natural cycle.** Don't invent a boundary the business does
//!   not recognise just to shorten a stream.
//! - **Periods overlap.** One register used by several waiters at once has no
//!   single clean "close" — the lifecycle isn't sequential.
//! - **You can't produce predictable ids** for the next period.
//! - **The stream is low-frequency.** A rarely-changing entity (a company
//!   address) does not grow fast enough for length to matter.
//!
//! ## References
//!
//! - Oskar Dudycz, "Closing the Books in Practice":
//!   <https://event-driven.io/en/closing_the_books_in_practice/>
//! - Oskar Dudycz, "Should you always keep streams short in Event Sourcing?":
//!   <https://event-driven.io/en/should_you_always_keep_streams_short/>
//! - Kurrent (`EventStoreDB`), "Snapshots in Event Sourcing":
//!   <https://www.kurrent.io/blog/snapshots-in-event-sourcing/>
//! - Kurrent (`EventStoreDB`), "How to Model Event-Sourced Systems Efficiently":
//!   <https://www.kurrent.io/blog/how-to-model-event-sourced-systems-efficiently/>
