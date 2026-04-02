/// DomainEvent derive must reject structs.

#[derive(Debug, nexus::DomainEvent)]
struct NotAnEnum {
    name: String,
}

fn main() {}
