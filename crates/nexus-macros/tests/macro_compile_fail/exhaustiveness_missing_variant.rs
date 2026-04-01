/// When a new event variant is added, the compiler must force
/// the user to handle it in AggregateState::apply().
/// This is the core type safety guarantee of concrete event enums.

use nexus::*;

#[derive(Debug, Clone, nexus::DomainEvent)]
enum OrderEvent {
    Created(String),
    Shipped,
    Cancelled, // new variant added
}

#[derive(Default, Debug)]
struct OrderState {
    status: String,
}

impl AggregateState for OrderState {
    type Event = OrderEvent;

    fn apply(&mut self, event: &OrderEvent) {
        match event {
            OrderEvent::Created(name) => self.status = name.clone(),
            OrderEvent::Shipped => self.status = "shipped".into(),
            // Missing: OrderEvent::Cancelled — MUST fail
        }
    }

    fn name(&self) -> &'static str {
        "Order"
    }
}

fn main() {}
