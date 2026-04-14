//! In-memory event-sourced bank account example.
//!
//! Demonstrates the full Nexus pattern without any persistence layer:
//! - Domain events as enums with `#[derive(DomainEvent)]`
//! - Aggregate state with exhaustive event handling
//! - Command handling via `Handle<C>` trait
//! - Event replay (rehydration) from in-memory store
//! - Multiple aggregates in the same system

// Relaxed lints for example code — production crates should NOT do this.
#![allow(clippy::unwrap_used, reason = "example code uses unwrap for brevity")]
#![allow(clippy::expect_used, reason = "example code uses expect for clarity")]
#![allow(
    clippy::print_stdout,
    reason = "example code prints to demonstrate output"
)]
#![allow(
    clippy::str_to_string,
    reason = "example code uses to_string for readability"
)]

use nexus::*;
use std::collections::HashMap;
use std::fmt;

// =============================================================================
// Domain: Bank Account
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

impl AsRef<[u8]> for AccountId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

// --- Events ---

#[derive(Debug, Clone)]
struct AccountOpened {
    owner: String,
}

#[derive(Debug, Clone)]
struct MoneyDeposited {
    amount: u64,
}

#[derive(Debug, Clone)]
struct MoneyWithdrawn {
    amount: u64,
}

#[derive(Debug, Clone)]
struct AccountClosed;

#[derive(Debug, Clone, DomainEvent)]
enum AccountEvent {
    Opened(AccountOpened),
    Deposited(MoneyDeposited),
    Withdrawn(MoneyWithdrawn),
    Closed(AccountClosed),
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
            AccountEvent::Closed(_) => {
                self.is_open = false;
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
    #[error("cannot close account with balance {0}")]
    NonZeroBalance(u64),
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

struct CloseAccount;

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

impl Handle<CloseAccount> for BankAccount {
    fn handle(&self, _cmd: CloseAccount) -> Result<Events<AccountEvent>, AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        if self.state().balance > 0 {
            return Err(AccountError::NonZeroBalance(self.state().balance));
        }
        Ok(events![AccountEvent::Closed(AccountClosed)])
    }
}

// =============================================================================
// In-Memory Event Store (not part of nexus — just for this example)
// =============================================================================

struct InMemoryStore {
    streams: HashMap<AccountId, Vec<VersionedEvent<AccountEvent>>>,
}

impl InMemoryStore {
    fn new() -> Self {
        Self {
            streams: HashMap::new(),
        }
    }

    /// Simulate persist + apply: store the decided events, advance version,
    /// and apply to in-memory state.
    fn save(&mut self, account: &mut BankAccount, decided: &Events<AccountEvent>) {
        let stream = self.streams.entry(account.id().clone()).or_default();
        let base = account.version().map_or(0, |v| v.as_u64());
        for (i, event) in decided.iter().enumerate() {
            let ver = Version::new(base + u64::try_from(i).unwrap() + 1).unwrap();
            stream.push(VersionedEvent::new(ver, event.clone()));
        }
        let new_version = Version::new(base + u64::try_from(decided.len()).unwrap()).unwrap();
        account.root_mut().advance_version(new_version);
        account.root_mut().apply_events(decided);
    }

    fn load(&self, id: &AccountId) -> Option<BankAccount> {
        let events = self.streams.get(id)?;
        let mut account = BankAccount::new(id.clone());
        for e in events {
            account
                .root_mut()
                .replay(e.version(), e.event())
                .expect("valid event sequence");
        }
        Some(account)
    }
}

// =============================================================================
// Main
// =============================================================================

fn main() {
    let mut store = InMemoryStore::new();
    let alice_id = AccountId("alice-001".into());
    let bob_id = AccountId("bob-001".into());

    // --- Alice opens an account and deposits money ---
    println!("=== Alice's Account ===");

    let mut alice = BankAccount::new(alice_id.clone());
    let decided = alice
        .handle(OpenAccount {
            owner: "Alice Smith".into(),
        })
        .expect("open");
    store.save(&mut alice, &decided);

    let decided = alice.handle(Deposit { amount: 1000 }).expect("deposit");
    store.save(&mut alice, &decided);

    let decided = alice.handle(Deposit { amount: 500 }).expect("deposit");
    store.save(&mut alice, &decided);

    println!(
        "Balance: {} (version: {:?})",
        alice.state().balance,
        alice.version()
    );

    // --- Bob opens an account ---
    println!("\n=== Bob's Account ===");

    let mut bob = BankAccount::new(bob_id.clone());
    let decided = bob
        .handle(OpenAccount {
            owner: "Bob Jones".into(),
        })
        .expect("open");
    store.save(&mut bob, &decided);

    let decided = bob.handle(Deposit { amount: 200 }).expect("deposit");
    store.save(&mut bob, &decided);

    println!(
        "Balance: {} (version: {:?})",
        bob.state().balance,
        bob.version()
    );

    // --- Reload Alice from the store (rehydration) ---
    println!("\n=== Reload Alice from Store ===");

    let mut alice = store.load(&alice_id).expect("alice exists");
    println!(
        "Rehydrated: owner={}, balance={}, version={:?}",
        alice.state().owner,
        alice.state().balance,
        alice.version()
    );

    // --- Alice withdraws and closes ---
    let decided = alice.handle(Withdraw { amount: 300 }).expect("withdraw");
    store.save(&mut alice, &decided);
    println!("After withdrawal: balance={}", alice.state().balance);

    // Try to overdraw
    let err = alice
        .handle(Withdraw { amount: 5000 })
        .expect_err("overdraw");
    println!("Overdraw rejected: {err}");

    // Withdraw remaining and close
    let decided = alice
        .handle(Withdraw { amount: 1200 })
        .expect("withdraw remaining");
    store.save(&mut alice, &decided);

    let decided = alice.handle(CloseAccount).expect("close");
    store.save(&mut alice, &decided);

    println!(
        "Closed: is_open={}, version={:?}",
        alice.state().is_open,
        alice.version()
    );

    // --- Try to deposit to closed account ---
    let alice = store.load(&alice_id).expect("alice exists");
    let err = alice.handle(Deposit { amount: 100 }).expect_err("closed");
    println!("Deposit to closed account rejected: {err}");

    // --- Final state ---
    println!("\n=== Final Store State ===");
    for (id, events) in &store.streams {
        println!("{id}: {} events", events.len());
    }
}
