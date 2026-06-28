//! Shared domain for the end-to-end example: a tiny event-sourced bank
//! account (the same shape as `examples/inmemory`, with `serde` derives added
//! so the built-in `JsonCodec` can persist it). Reused across all three phases
//! so the example demonstrates *one* domain flowing through persistence,
//! subscription, and backup/restore.

use std::fmt;

use nexus::{AggregateState, DomainEvent, Events, Handle, Id, events};
use serde::{Deserialize, Serialize};

// ── Id ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AccountId(pub String);

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<[u8]> for AccountId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Id for AccountId {
    const BYTE_LEN: usize = 0;
}

// ── Events (serde-encodable so the built-in JsonCodec can persist them) ──────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountOpened {
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoneyDeposited {
    pub amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoneyWithdrawn {
    pub amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, DomainEvent)]
pub enum AccountEvent {
    Opened(AccountOpened),
    Deposited(MoneyDeposited),
    Withdrawn(MoneyWithdrawn),
}

// ── State ───────────────────────────────────────────────────────────────────

/// `PartialEq`/`Eq` so the example can assert round-trip *aggregate equality*
/// (rehydrated state `==` original) after a reopen or a backup/restore.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct AccountState {
    pub owner: String,
    pub balance: u64,
    pub is_open: bool,
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
            AccountEvent::Deposited(e) => self.balance += e.amount,
            AccountEvent::Withdrawn(e) => self.balance -= e.amount,
        }
        self
    }
}

// ── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("account already open")]
    AlreadyOpen,
    #[error("account is closed")]
    Closed,
    #[error("insufficient funds: have {balance}, need {amount}")]
    InsufficientFunds { balance: u64, amount: u64 },
}

// ── Aggregate marker + commands ─────────────────────────────────────────────

#[nexus::aggregate(state = AccountState, error = AccountError, id = AccountId)]
pub struct BankAccount;

pub struct OpenAccount {
    pub owner: String,
}
pub struct Deposit {
    pub amount: u64,
}
pub struct Withdraw {
    pub amount: u64,
}

impl Handle<OpenAccount> for BankAccount {
    fn handle(
        state: &AccountState,
        cmd: OpenAccount,
    ) -> Result<Events<AccountEvent>, AccountError> {
        if state.is_open {
            return Err(AccountError::AlreadyOpen);
        }
        Ok(events![AccountEvent::Opened(AccountOpened {
            owner: cmd.owner
        })])
    }
}

impl Handle<Deposit> for BankAccount {
    fn handle(state: &AccountState, cmd: Deposit) -> Result<Events<AccountEvent>, AccountError> {
        if !state.is_open {
            return Err(AccountError::Closed);
        }
        Ok(events![AccountEvent::Deposited(MoneyDeposited {
            amount: cmd.amount
        })])
    }
}

impl Handle<Withdraw> for BankAccount {
    fn handle(state: &AccountState, cmd: Withdraw) -> Result<Events<AccountEvent>, AccountError> {
        if !state.is_open {
            return Err(AccountError::Closed);
        }
        if state.balance < cmd.amount {
            return Err(AccountError::InsufficientFunds {
                balance: state.balance,
                amount: cmd.amount,
            });
        }
        Ok(events![AccountEvent::Withdrawn(MoneyWithdrawn {
            amount: cmd.amount
        })])
    }
}
