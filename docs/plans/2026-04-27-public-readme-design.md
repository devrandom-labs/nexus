# Public README Design

## Goal

Make the nexus repository public-ready. Update all READMEs to reflect the current API and position Nexus for its target audience: Rust developers who already know event sourcing and DDD.

## Scope

1. Root `README.md` — full rewrite
2. `crates/nexus/README.md` — update to current API
3. `crates/nexus-macros/README.md` — fix to show actual macros
4. Flip repo visibility to public via `gh repo edit --visibility public`

## Positioning

Nexus as a standalone event-sourcing framework. No mention of downstream systems (Agency, Zenoh, AI agents). Lead with code quality — the API speaks for itself.

## Target Audience

Experienced Rust developers who already know ES/DDD/CQRS patterns. No hand-holding on concepts. They want to know why *this* library.

## Root README Structure

### 1. Hook + Badges

One-liner: "Event sourcing for Rust — no `Box<dyn>`, no runtime downcasting, no hidden allocations."

CI and license badges only. No crates.io/docs.rs badges until published.

### 2. Code Example (lead section)

Trimmed bank account example (~35 lines) showing:
- `#[derive(DomainEvent)]` on a flat enum (no inner structs for brevity)
- `AggregateState` with `apply(self, &Event) -> Self` (value-semantic signature)
- `#[nexus::aggregate(state = S, error = E, id = I)]` macro
- `Handle<Deposit>` impl with `events![]` macro

Code-first approach: experienced devs read the API surface before marketing copy.

### 3. "Why Nexus?" — Three Differentiators

Three paragraphs, each: **bold claim** + mechanism:

1. **Concrete event enums, not trait objects.** Exhaustive `match`, compiler catches missing handlers.
2. **`no_std` / `no_alloc` kernel.** `ArrayVec` + const generics. Zero heap in the kernel.
3. **Zero-copy read path.** `BorrowingCodec` + GAT lending iterators.

### 4. Crates Table

Four crates: `nexus`, `nexus-macros`, `nexus-store`, `nexus-fjall`. Relative links to crate directories (not crates.io — not published yet).

### 5. Features List

Compact single-line items for supporting differentiators:
- Schema evolution (`#[nexus::transforms]`)
- Aggregate snapshots
- Optimistic concurrency
- Allocation-free errors
- Strict verification (proptest, miri, mutation testing, trybuild)

### 6. Getting Started

`Cargo.toml` snippets for kernel-only and with persistence. Links to the three examples.

### 7. Status

Honest: experimental, unstable API, kernel well-tested, store/fjall under active development.

### 8. License

MIT OR Apache-2.0.

## Crate READMEs

### `nexus/README.md`

Strip to: hook line, link to root README, verification table (kept — it's specific to the kernel and impressive).

### `nexus-macros/README.md`

Fix to show the three actual macros with correct syntax:
- `#[nexus::aggregate(state = S, error = E, id = I)]` — attribute macro on unit struct
- `#[derive(DomainEvent)]` — derive on enum
- `#[nexus::transforms(aggregate = A, error = E)]` — attribute macro on impl block

Kill `Command` and `Query` references (they don't exist).

## What's NOT Changing

- `CONTRIBUTING.md` — already adequate
- `CLAUDE.md` — internal, not public-facing
- Examples — already compile and reflect current API
