//! Store-and-kernel example: full event-sourcing lifecycle.
//!
//! Demonstrates integrating the Nexus kernel (aggregates, events, state)
//! with the nexus-store persistence layer (codec, raw event store, event stream).
//!
//! Flow:
//! 1. Define a BankAccount aggregate using `#[nexus::aggregate]`
//! 2. Use a JSON `Codec<AccountEvent>` for serialization
//! 3. Use an in-memory `RawEventStore` for persistence
//! 4. Show: create aggregate -> apply events -> encode -> persist -> read back
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
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::stream::EventStream;
use nexus_store::{Codec, RawEventStore, pending_envelope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use tokio::sync::Mutex;

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

impl Id for AccountId {}

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

#[derive(Default, Debug)]
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

    fn apply(&mut self, event: &AccountEvent) {
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
    }

    fn name(&self) -> &'static str {
        "BankAccount"
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

// --- Business logic ---

impl BankAccount {
    fn open(&mut self, owner: String) -> Result<(), AccountError> {
        if self.state().is_open {
            return Err(AccountError::AlreadyOpen);
        }
        self.apply(AccountEvent::Opened(AccountOpened { owner }));
        Ok(())
    }

    fn deposit(&mut self, amount: u64) -> Result<(), AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        self.apply(AccountEvent::Deposited(MoneyDeposited { amount }));
        Ok(())
    }

    fn withdraw(&mut self, amount: u64) -> Result<(), AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        if self.state().balance < amount {
            return Err(AccountError::InsufficientFunds {
                balance: self.state().balance,
                amount,
            });
        }
        self.apply(AccountEvent::Withdrawn(MoneyWithdrawn { amount }));
        Ok(())
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
// In-Memory Raw Event Store (nexus-store adapter)
// =============================================================================

/// Row stored per event: (stream_id, version, event_type, payload).
type StoredRow = (String, u64, String, Vec<u8>);

/// Minimal in-memory adapter implementing `RawEventStore`.
/// Stores raw bytes — knows nothing about domain events.
struct InMemStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

/// Lending cursor for reading events back from the in-memory store.
struct InMemStream {
    events: Vec<StoredRow>,
    pos: usize,
}

impl EventStream for InMemStream {
    type Error = StoreError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            1,
            &row.3,
            (),
        )))
    }
}

#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("concurrency conflict: expected version {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
}

impl RawEventStore for InMemStore {
    type Error = StoreError;
    type Stream<'a>
        = InMemStream
    where
        Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current_version = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_version != expected_version.as_u64() {
            return Err(StoreError::Conflict {
                expected: expected_version.as_u64(),
                actual: current_version,
            });
        }
        for env in envelopes {
            stream.push((
                stream_id.to_owned(),
                env.version().as_u64(),
                env.event_type().to_owned(),
                env.payload().to_vec(),
            ));
        }
        drop(guard);
        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self
            .streams
            .lock()
            .await
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|(_, v, _, _)| *v >= from.as_u64())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Ok(InMemStream { events, pos: 0 })
    }
}

// =============================================================================
// Bridge functions: kernel <-> store
// =============================================================================

/// Kernel -> Store: encode uncommitted events into pending envelopes.
fn encode_events(
    codec: &JsonCodec,
    stream_id: &str,
    uncommitted: &[VersionedEvent<AccountEvent>],
) -> Vec<PendingEnvelope<()>> {
    uncommitted
        .iter()
        .map(|ve| {
            let payload = codec.encode(ve.event()).expect("encode should succeed");
            pending_envelope(stream_id.to_owned())
                .version(ve.version())
                .event_type(ve.event().name())
                .payload(payload)
                .build_without_metadata()
        })
        .collect()
}

/// Store -> Kernel: read stream, decode each envelope, build versioned events.
async fn load_events(
    codec: &JsonCodec,
    store: &InMemStore,
    stream_id: &str,
) -> Vec<VersionedEvent<AccountEvent>> {
    let mut stream = store
        .read_stream(stream_id, Version::from_persisted(1))
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
                versioned.push(VersionedEvent::from_persisted(env.version(), event));
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
    let store = InMemStore::new();
    let stream_id = "account-alice";

    // -------------------------------------------------------------------------
    // Step 1: Create aggregate, apply business operations
    // -------------------------------------------------------------------------
    println!("=== Step 1: Create and operate on BankAccount ===");

    let alice_id = AccountId("alice".to_owned());
    let mut account = BankAccount::new(alice_id.clone());
    account
        .open("Alice Smith".to_owned())
        .expect("open should succeed");
    account.deposit(1000).expect("deposit should succeed");
    account.deposit(500).expect("deposit should succeed");

    println!(
        "State: owner={}, balance={}, version={}",
        account.state().owner,
        account.state().balance,
        account.current_version()
    );

    // -------------------------------------------------------------------------
    // Step 2: Take uncommitted events, encode, persist to store
    // -------------------------------------------------------------------------
    println!("\n=== Step 2: Persist events to store ===");

    let uncommitted = account.take_uncommitted_events();
    let envelopes = encode_events(&codec, stream_id, &uncommitted);
    store
        .append(stream_id, Version::INITIAL, &envelopes)
        .await
        .expect("append should succeed");

    println!(
        "Persisted {} events to stream '{stream_id}'",
        envelopes.len()
    );

    // -------------------------------------------------------------------------
    // Step 3: Read back from store, decode, rehydrate into new aggregate
    // -------------------------------------------------------------------------
    println!("\n=== Step 3: Rehydrate from store ===");

    let versioned = load_events(&codec, &store, stream_id).await;
    let mut account = BankAccount::new(alice_id.clone());
    for ve in versioned {
        let (version, event) = ve.into_parts();
        account
            .root_mut()
            .replay(version, &event)
            .expect("rehydration should succeed");
    }

    println!(
        "Rehydrated: owner={}, balance={}, version={}",
        account.state().owner,
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 4: More operations, persist again (demonstrating optimistic concurrency)
    // -------------------------------------------------------------------------
    println!("\n=== Step 4: Withdraw and persist more events ===");

    let mut account = account;
    let expected_version = account.version();
    account.withdraw(300).expect("withdraw should succeed");

    let uncommitted = account.take_uncommitted_events();
    let envelopes = encode_events(&codec, stream_id, &uncommitted);
    store
        .append(stream_id, expected_version, &envelopes)
        .await
        .expect("append should succeed");

    println!(
        "Persisted {} more event(s). balance={}, version={}",
        envelopes.len(),
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 5: Full reload — verify entire history
    // -------------------------------------------------------------------------
    println!("\n=== Step 5: Full reload from store ===");

    let versioned = load_events(&codec, &store, stream_id).await;
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
        "Final state: owner={}, balance={}, version={}",
        account.state().owner,
        account.state().balance,
        account.version()
    );

    // -------------------------------------------------------------------------
    // Step 6: Demonstrate invariant enforcement
    // -------------------------------------------------------------------------
    println!("\n=== Step 6: Invariant enforcement ===");

    let mut account = account;
    let err = account.withdraw(5000).expect_err("should reject overdraw");
    println!("Overdraw rejected: {err}");

    let err = account.open("Bob".to_owned()).expect_err("already open");
    println!("Double-open rejected: {err}");

    println!("\nDone. Full event-sourcing lifecycle with kernel + store integration.");
}
