//! Closing the Books — bounded streams vs. a long-lived aggregate.
//!
//! Builds the same cash-register domain two ways and prints how many events
//! each must replay to read the current float. See the `nexus::closing_the_books`
//! module for the narrative.

// Relaxed lints for example code — production crates should NOT do this.
#![allow(clippy::unwrap_used, reason = "example code uses unwrap for brevity")]
#![allow(clippy::expect_used, reason = "example code uses expect for clarity")]
#![allow(
    clippy::print_stdout,
    reason = "example code prints to demonstrate output"
)]

use nexus::*;
use std::fmt;

// =============================================================================
// Shared helpers (a tiny stand-in for what a repository does)
// =============================================================================

/// Append decided events to a stream history and advance the in-memory root.
/// This stands in for a real `Repository`, mirroring its persist-then-apply
/// step by driving the `advance_version` / `apply_events` mutators directly.
fn record<A: Aggregate, const N: usize>(
    root: &mut AggregateRoot<A>,
    history: &mut Vec<VersionedEvent<EventOf<A>>>,
    decided: &Events<EventOf<A>, N>,
) where
    EventOf<A>: Clone,
{
    let base = root.version().map_or(0, |v| v.as_u64());
    for (i, event) in decided.iter().enumerate() {
        let version = Version::new(base + u64::try_from(i).unwrap() + 1).unwrap();
        history.push(VersionedEvent::new(version, event.clone()));
    }
    let new_version = Version::new(base + u64::try_from(decided.len()).unwrap()).unwrap();
    root.advance_version(new_version);
    root.apply_events(decided);
}

/// Rebuild current state from one stream by replaying every event, returning
/// the rehydrated root. The pattern's payoff shows up at the call site: a
/// bounded stream means only a handful of events to replay here.
fn replay_stream<A: Aggregate>(
    id: A::Id,
    history: &[VersionedEvent<EventOf<A>>],
) -> AggregateRoot<A> {
    let mut root = AggregateRoot::<A>::new(id);
    for versioned in history {
        root.replay(versioned.version(), versioned.event())
            .expect("valid history");
    }
    root
}

// =============================================================================
// Pattern: CashierShift — a bounded, lifecycle-scoped stream
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CashierShiftId {
    register: String,
    shift_number: u32,
    urn: String,
}

impl CashierShiftId {
    fn new(register: &str, shift_number: u32) -> Self {
        Self {
            register: register.to_owned(),
            shift_number,
            urn: format!("urn:cashier_shift:{register}:{shift_number}"),
        }
    }
}

impl fmt::Display for CashierShiftId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.urn)
    }
}

impl Id for CashierShiftId {
    const BYTE_LEN: usize = 0;
}

impl AsRef<[u8]> for CashierShiftId {
    fn as_ref(&self) -> &[u8] {
        self.urn.as_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, DomainEvent)]
enum ShiftEvent {
    Opened(ShiftOpened),
    TransactionRegistered(TransactionRegistered),
    Closed(ShiftClosed),
}

#[derive(Debug, Clone, PartialEq)]
struct ShiftOpened {
    opening_float: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct TransactionRegistered {
    amount: u64,
}

/// The summary ("closing the books") event: a first-class domain event that
/// carries the minimum state the next shift needs to open.
#[derive(Debug, Clone, PartialEq)]
struct ShiftClosed {
    declared_tender: u64,
    overage: u64,
    shortage: u64,
    final_float: u64,
}

#[derive(Default, Debug, Clone)]
struct ShiftState {
    opening_float: u64,
    registered_total: u64,
    is_open: bool,
    closed: bool,
}

impl AggregateState for ShiftState {
    type Event = ShiftEvent;

    fn initial() -> Self {
        Self::default()
    }

    fn apply(mut self, event: &ShiftEvent) -> Self {
        match event {
            ShiftEvent::Opened(e) => {
                self.opening_float = e.opening_float;
                self.registered_total = 0;
                self.is_open = true;
            }
            // Example values stay small; production aggregates use checked_add.
            ShiftEvent::TransactionRegistered(e) => {
                self.registered_total += e.amount;
            }
            ShiftEvent::Closed(_) => {
                self.is_open = false;
                self.closed = true;
            }
        }
        self
    }
}

#[nexus::aggregate(state = ShiftState, error = ShiftError, id = CashierShiftId)]
struct CashierShift;

#[derive(Debug, thiserror::Error, PartialEq)]
enum ShiftError {
    #[error("shift already opened")]
    AlreadyOpen,
    #[error("shift is not open")]
    NotOpen,
}

struct OpenShift {
    opening_float: u64,
}

struct RegisterTransaction {
    amount: u64,
}

struct CloseShift {
    declared_tender: u64,
}

impl Handle<OpenShift> for CashierShift {
    fn handle(state: &ShiftState, cmd: OpenShift) -> Result<Events<ShiftEvent>, ShiftError> {
        if state.is_open || state.closed {
            return Err(ShiftError::AlreadyOpen);
        }
        Ok(events![ShiftEvent::Opened(ShiftOpened {
            opening_float: cmd.opening_float,
        })])
    }
}

impl Handle<RegisterTransaction> for CashierShift {
    fn handle(
        state: &ShiftState,
        cmd: RegisterTransaction,
    ) -> Result<Events<ShiftEvent>, ShiftError> {
        if !state.is_open {
            return Err(ShiftError::NotOpen);
        }
        Ok(events![ShiftEvent::TransactionRegistered(
            TransactionRegistered { amount: cmd.amount }
        )])
    }
}

impl Handle<CloseShift> for CashierShift {
    fn handle(state: &ShiftState, cmd: CloseShift) -> Result<Events<ShiftEvent>, ShiftError> {
        if !state.is_open {
            return Err(ShiftError::NotOpen);
        }
        let expected = state.opening_float + state.registered_total;
        let overage = if cmd.declared_tender > expected {
            cmd.declared_tender - expected
        } else {
            0
        };
        let shortage = if expected > cmd.declared_tender {
            expected - cmd.declared_tender
        } else {
            0
        };
        Ok(events![ShiftEvent::Closed(ShiftClosed {
            declared_tender: cmd.declared_tender,
            overage,
            shortage,
            final_float: cmd.declared_tender,
        })])
    }
}

fn run_cashier_shift_demo() {
    println!("=== Closing the Books: CashierShift (bounded streams) ===");

    // Shift #1: open with float 100, register 10 sales of 1 each, then close.
    let shift1_id = CashierShiftId::new("till-1", 1);
    let mut shift1 = AggregateRoot::<CashierShift>::new(shift1_id.clone());
    let mut shift1_stream: Vec<VersionedEvent<ShiftEvent>> = Vec::new();

    let opened = shift1
        .handle(OpenShift { opening_float: 100 })
        .expect("open shift 1");
    record(&mut shift1, &mut shift1_stream, &opened);
    for _ in 0..10 {
        let txn = shift1
            .handle(RegisterTransaction { amount: 1 })
            .expect("register txn");
        record(&mut shift1, &mut shift1_stream, &txn);
    }
    let closed = shift1
        .handle(CloseShift {
            declared_tender: 110,
        })
        .expect("close shift 1");
    // The summary event carries the closing float forward — read it from the
    // decided ShiftClosed event rather than recomputing it.
    let final_float = closed
        .iter()
        .find_map(|e| match e {
            ShiftEvent::Closed(c) => Some(c.final_float),
            ShiftEvent::Opened(_) | ShiftEvent::TransactionRegistered(_) => None,
        })
        .expect("close produced a ShiftClosed event");
    record(&mut shift1, &mut shift1_stream, &closed);
    println!(
        "shift #1 closed: stream length = {}, final_float = {final_float}",
        shift1_stream.len()
    );

    // Shift #2: a brand-new stream, opened from the carried-forward float.
    let shift2_id = CashierShiftId::new(&shift1_id.register, shift1_id.shift_number + 1);
    let mut shift2 = AggregateRoot::<CashierShift>::new(shift2_id.clone());
    let mut shift2_stream: Vec<VersionedEvent<ShiftEvent>> = Vec::new();

    let opened = shift2
        .handle(OpenShift {
            opening_float: final_float,
        })
        .expect("open shift 2");
    record(&mut shift2, &mut shift2_stream, &opened);
    for _ in 0..10 {
        let txn = shift2
            .handle(RegisterTransaction { amount: 1 })
            .expect("register txn");
        record(&mut shift2, &mut shift2_stream, &txn);
    }

    // Read current state by replaying ONLY shift #2's short stream — no earlier
    // shift is touched. The recovered total confirms the carry-forward worked.
    let shift2_root = replay_stream::<CashierShift>(shift2_id.clone(), &shift2_stream);
    println!("shift #2 {shift2_id} opened from carried float {final_float}");
    println!(
        "replaying ONLY the current shift ({} events) recovers registered total = {}",
        shift2_stream.len(),
        shift2_root.state().registered_total,
    );
}

fn main() {
    run_cashier_shift_demo();
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus::testing::AggregateFixture;

    fn fixture() -> AggregateFixture<CashierShift> {
        AggregateFixture::with_id(CashierShiftId::new("till-1", 1))
    }

    #[test]
    fn close_with_exact_tender_has_no_discrepancy() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 150,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 150,
                overage: 0,
                shortage: 0,
                final_float: 150,
            })]);
    }

    #[test]
    fn close_with_surplus_reports_overage() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 160,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 160,
                overage: 10,
                shortage: 0,
                final_float: 160,
            })]);
    }

    #[test]
    fn close_with_deficit_reports_shortage() {
        let _ = fixture()
            .given([
                ShiftEvent::Opened(ShiftOpened { opening_float: 100 }),
                ShiftEvent::TransactionRegistered(TransactionRegistered { amount: 50 }),
            ])
            .when(CloseShift {
                declared_tender: 140,
            })
            .then_expect_events([ShiftEvent::Closed(ShiftClosed {
                declared_tender: 140,
                overage: 0,
                shortage: 10,
                final_float: 140,
            })]);
    }

    #[test]
    fn close_before_open_is_rejected() {
        let _ = fixture()
            .given(Vec::<ShiftEvent>::new())
            .when(CloseShift {
                declared_tender: 100,
            })
            .then_expect_error(ShiftError::NotOpen);
    }
}
