/// DomainEvent enum must have at least one variant.

#[derive(Debug, Clone, nexus::DomainEvent)]
enum EmptyEvent {}

fn main() {}
