//! In-memory event-sourced bank account example.
//!
//! Demonstrates the full Nexus pattern without any persistence layer:
//! - Domain events as enums with #[derive(DomainEvent)]
//! - Aggregate state with exhaustive event handling
//! - Aggregate with business logic and invariant enforcement
//! - Event replay (rehydration) from in-memory store
//! - Multiple aggregates in the same system

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
            AccountEvent::Closed(_) => {
                self.is_open = false;
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
    #[error("cannot close account with balance {0}")]
    NonZeroBalance(u64),
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

    fn close(&mut self) -> Result<(), AccountError> {
        if !self.state().is_open {
            return Err(AccountError::Closed);
        }
        if self.state().balance > 0 {
            return Err(AccountError::NonZeroBalance(self.state().balance));
        }
        self.apply(AccountEvent::Closed(AccountClosed));
        Ok(())
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

    fn save(&mut self, account: &mut BankAccount) {
        let events = account.take_uncommitted_events();
        let stream = self.streams.entry(account.id().clone()).or_default();
        for event in events {
            stream.push(VersionedEvent::from_persisted(
                event.version(),
                event.event().clone(),
            ));
        }
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
    alice.open("Alice Smith".into()).expect("open");
    alice.deposit(1000).expect("deposit");
    alice.deposit(500).expect("deposit");
    store.save(&mut alice);

    println!(
        "Balance: {} (version: {})",
        alice.state().balance,
        alice.version()
    );

    // --- Bob opens an account ---
    println!("\n=== Bob's Account ===");

    let mut bob = BankAccount::new(bob_id.clone());
    bob.open("Bob Jones".into()).expect("open");
    bob.deposit(200).expect("deposit");
    store.save(&mut bob);

    println!(
        "Balance: {} (version: {})",
        bob.state().balance,
        bob.version()
    );

    // --- Reload Alice from the store (rehydration) ---
    println!("\n=== Reload Alice from Store ===");

    let mut alice = store.load(&alice_id).expect("alice exists");
    println!(
        "Rehydrated: owner={}, balance={}, version={}",
        alice.state().owner,
        alice.state().balance,
        alice.version()
    );

    // --- Alice withdraws and closes ---
    alice.withdraw(300).expect("withdraw");
    println!("After withdrawal: balance={}", alice.state().balance);

    // Try to overdraw
    let err = alice.withdraw(5000).expect_err("overdraw");
    println!("Overdraw rejected: {err}");

    // Withdraw remaining and close
    alice.withdraw(1200).expect("withdraw remaining");
    alice.close().expect("close");
    store.save(&mut alice);

    println!(
        "Closed: is_open={}, version={}",
        alice.state().is_open,
        alice.version()
    );

    // --- Try to deposit to closed account ---
    let mut alice = store.load(&alice_id).expect("alice exists");
    let err = alice.deposit(100).expect_err("closed");
    println!("Deposit to closed account rejected: {err}");

    // --- Final state ---
    println!("\n=== Final Store State ===");
    for (id, events) in &store.streams {
        println!("{id}: {} events", events.len());
    }
}
