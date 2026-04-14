//! Store-and-kernel example: full event-sourcing lifecycle.
//!
//! Demonstrates integrating the Nexus kernel (aggregates, events, state)
//! with the nexus-store persistence layer (codec, raw event store, event stream).
//!
//! Flow:
//! 1. Define a BankAccount aggregate using `#[nexus::aggregate]`
//! 2. Use a JSON `Codec<AccountEvent>` for serialization
//! 3. Use `InMemoryStore` from nexus-store for persistence
//! 4. Show: create aggregate -> handle command -> encode -> persist -> read back
//!    -> decode -> rehydrate aggregate

// Example crate — relax strict lints for readability.
#![allow(clippy::unwrap_used, reason = "example uses unwrap for brevity")]
#![allow(clippy::expect_used, reason = "example uses expect for brevity")]
#![allow(clippy::panic, reason = "example may panic on unexpected errors")]
#![allow(clippy::print_stdout, reason = "example prints to stdout")]
#![allow(clippy::print_stderr, reason = "example prints to stderr")]
#![allow(clippy::str_to_string, reason = "example uses to_string freely")]
#![allow(clippy::shadow_reuse, reason = "example shadows for readability")]
#![allow(clippy::shadow_unrelated, reason = "example shadows for readability")]

use nexus::*;
use nexus_store::store::EventStream;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use nexus_store::{Codec, pending_envelope};
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Domain: Bank Account (kernel layer)
// =============================================================================

// --- ID ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AccountId(String);

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Id for AccountId {
    const BYTE_LEN: usize = 0;
}

impl AsRef<[u8]> for AccountId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

// --- Events (with serde for codec integration) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountOpened {
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MoneyDeposited {
    amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MoneyWithdrawn {
    amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, DomainEvent)]
enum AccountEvent {
    Opened(AccountOpened),
    Deposited(MoneyDeposited),
    Withdrawn(MoneyWithdrawn),
}

// --- State ---

#[derive(Default, Debug, Clone)]
struct AccountState {
    owner: String,
    balance: u64,
    is_open: bool,
}

impl AggregateState for AccountState {
    type Event = AccountEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &AccountEvent) -> Self {
        match event {
            AccountEvent::Opened(e) => {
                self.owner = e.owner.clone();
                self.is_open = true;
            }
            AccountEvent::Deposited(e) => {
                self.balance += e.amount;
            }
            AccountEvent::Withdrawn(e) => {
                self.balance -= e.amount;
            }
        }
        self
    }
}

// --- Errors ---

#[derive(Debug, thiserror::Error)]
enum AccountError {
    #[error("account already open")]
    AlreadyOpen,
    #[error("account is closed")]
    Closed,
    #[error("insufficient funds: have {balance}, need {amount}")]
    InsufficientFunds { balance: u64, amount: u64 },
}

// --- Aggregate ---

#[nexus::aggregate(state = AccountState, error = AccountError, id = AccountId)]
struct BankAccount;

// --- Commands ---

struct OpenAccount {
    owner: String,
}

struct Deposit {
    amount: u64,
}

struct Withdraw {
    amount: u64,
}

impl Handle<OpenAccount> for BankAccount {
    fn handle(&self, cmd: OpenAccount) -> Result<Events<AccountEvent>, AccountError> {
        if self.state().is_open {
            return Err(AccountError::AlreadyOpen);
        }
        Ok(events![AccountEvent::Opened(AccountOpened {
            owner: cmd.owner,
        })])
    }
}

impl Handle<Deposit> for BankAccount {
    fn handle(&self, cmd: Deposit) -> Result<Events<AccountEvent>, AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        Ok(events![AccountEvent::Deposited(MoneyDeposited {
            amount: cmd.amount,
        })])
    }
}

impl Handle<Withdraw> for BankAccount {
    fn handle(&self, cmd: Withdraw) -> Result<Events<AccountEvent>, AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        if self.state().balance < cmd.amount {
            return Err(AccountError::InsufficientFunds {
                balance: self.state().balance,
                amount: cmd.amount,
            });
        }
        Ok(events![AccountEvent::Withdrawn(MoneyWithdrawn {
            amount: cmd.amount,
        })])
    }
}

// =============================================================================
// JSON Codec (bridge between kernel and store)
// =============================================================================

/// JSON codec for `AccountEvent` using serde_json.
struct JsonCodec;

impl Codec<AccountEvent> for JsonCodec {
    type Error = serde_json::Error;

    fn encode(&self, event: &AccountEvent) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(event)
    }

    fn decode(&self, _event_type: &str, payload: &[u8]) -> Result<AccountEvent, Self::Error> {
        serde_json::from_slice(payload)
    }
}

// =============================================================================
// Bridge functions: kernel <-> store
// =============================================================================

/// Encode decided events into pending envelopes for the store.
fn encode_decided(
    codec: &JsonCodec,
    decided: &Events<AccountEvent>,
    base_version: u64,
) -> Vec<nexus_store::envelope::PendingEnvelope<()>> {
    decided
        .iter()
        .enumerate()
        .map(|(i, event)| {
            let ver = Version::new(base_version + u64::try_from(i).unwrap() + 1).unwrap();
            let payload = codec.encode(event).expect("encode should succeed");
            pending_envelope(ver)
                .event_type(event.name())
                .payload(payload)
                .build_without_metadata()
        })
        .collect()
}

/// Read events from the store, decode, and build versioned events.
async fn load_events(
    codec: &JsonCodec,
    store: &InMemoryStore,
    id: &AccountId,
) -> Vec<VersionedEvent<AccountEvent>> {
    let mut stream = store
        .read_stream(id, Version::INITIAL)
        .await
        .expect("read_stream should succeed");

    let mut versioned = Vec::new();
    loop {
        let envelope = stream.next().await;
        match envelope {
            None => break,
            Some(result) => {
                let env = result.expect("stream read should succeed");
                let event: AccountEvent = codec
                    .decode(env.event_type(), env.payload())
                    .expect("decode should succeed");
                versioned.push(VersionedEvent::new(env.version(), event));
            }
        }
    }
    versioned
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() {
    let codec = JsonCodec;
    let store = InMemoryStore::new();

    // -------------------------------------------------------------------------
    // Step 1: Create aggregate, handle commands
    // -------------------------------------------------------------------------
    println!("=== Step 1: Create and operate on BankAccount ===");

    let alice_id = AccountId("alice".to_owned());
    let mut account = BankAccount::new(alice_id.clone());

    let decided = account
        .handle(OpenAccount {
            owner: "Alice Smith".to_owned(),
        })
        .expect("open should succeed");
    account.root_mut().apply_events(&decided);
    account.root_mut().advance_version(Version::new(1).unwrap());

    let decided = account
        .handle(Deposit { amount: 1000 })
        .expect("deposit should succeed");
    account.root_mut().apply_events(&decided);
    account.root_mut().advance_version(Version::new(2).unwrap());

    let decided = account
        .handle(Deposit { amount: 500 })
        .expect("deposit should succeed");
    account.root_mut().apply_events(&decided);
    account.root_mut().advance_version(Version::new(3).unwrap());

    println!(
        "State: owner={}, balance={}, version={:?}",
        account.state().owner,
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 2: Encode and persist events to store
    // -------------------------------------------------------------------------
    println!("\n=== Step 2: Persist events to store ===");

    // For this demo, re-handle all three commands and persist them.
    // In a real system, the repository handles this atomically.
    let mut fresh = BankAccount::new(alice_id.clone());

    let decided = fresh
        .handle(OpenAccount {
            owner: "Alice Smith".to_owned(),
        })
        .expect("open");
    let envelopes = encode_decided(&codec, &decided, 0);
    fresh.root_mut().apply_events(&decided);
    fresh.root_mut().advance_version(Version::new(1).unwrap());

    let decided = fresh.handle(Deposit { amount: 1000 }).expect("deposit");
    let mut more = encode_decided(&codec, &decided, 1);
    envelopes.len(); // keep envelopes alive
    fresh.root_mut().apply_events(&decided);
    fresh.root_mut().advance_version(Version::new(2).unwrap());

    let decided = fresh.handle(Deposit { amount: 500 }).expect("deposit");
    let mut even_more = encode_decided(&codec, &decided, 2);

    // Combine all envelopes and persist
    let mut all_envelopes = envelopes;
    all_envelopes.append(&mut more);
    all_envelopes.append(&mut even_more);

    store
        .append(&alice_id, None, &all_envelopes)
        .await
        .expect("append should succeed");

    println!("Persisted {} events to stream", all_envelopes.len());

    // -------------------------------------------------------------------------
    // Step 3: Read back from store, decode, rehydrate into new aggregate
    // -------------------------------------------------------------------------
    println!("\n=== Step 3: Rehydrate from store ===");

    let versioned = load_events(&codec, &store, &alice_id).await;
    let mut account = BankAccount::new(alice_id.clone());
    for ve in versioned {
        let (version, event) = ve.into_parts();
        account
            .root_mut()
            .replay(version, &event)
            .expect("rehydration should succeed");
    }

    println!(
        "Rehydrated: owner={}, balance={}, version={:?}",
        account.state().owner,
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 4: More operations, persist again (optimistic concurrency)
    // -------------------------------------------------------------------------
    println!("\n=== Step 4: Withdraw and persist more events ===");

    let expected_version = account.version();
    let decided = account
        .handle(Withdraw { amount: 300 })
        .expect("withdraw should succeed");
    let base = account.version().map_or(0, |v| v.as_u64());
    let envelopes = encode_decided(&codec, &decided, base);
    store
        .append(&alice_id, expected_version, &envelopes)
        .await
        .expect("append should succeed");

    account.root_mut().apply_events(&decided);
    let new_ver = Version::new(base + 1).unwrap();
    account.root_mut().advance_version(new_ver);

    println!(
        "Persisted {} more event(s). balance={}, version={:?}",
        envelopes.len(),
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 5: Full reload — verify entire history
    // -------------------------------------------------------------------------
    println!("\n=== Step 5: Full reload from store ===");

    let versioned = load_events(&codec, &store, &alice_id).await;
    println!("Total events in stream: {}", versioned.len());

    let mut account = BankAccount::new(alice_id);
    for ve in versioned {
        let (version, event) = ve.into_parts();
        account
            .root_mut()
            .replay(version, &event)
            .expect("rehydration should succeed");
    }

    println!(
        "Final state: owner={}, balance={}, version={:?}",
        account.state().owner,
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 6: Demonstrate invariant enforcement
    // -------------------------------------------------------------------------
    println!("\n=== Step 6: Invariant enforcement ===");

    let err = account
        .handle(Withdraw { amount: 5000 })
        .expect_err("should reject overdraw");
    println!("Overdraw rejected: {err}");

    let err = account
        .handle(OpenAccount {
            owner: "Bob".to_owned(),
        })
        .expect_err("already open");
    println!("Double-open rejected: {err}");

    println!("\nDone. Full event-sourcing lifecycle with kernel + store integration.");
}
