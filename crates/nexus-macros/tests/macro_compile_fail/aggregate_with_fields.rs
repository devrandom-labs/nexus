/// aggregate macro must reject structs with fields.

#[nexus::aggregate(state = (), error = (), id = ())]
struct HasFields {
    name: String,
}

fn main() {}
