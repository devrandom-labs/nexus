/// aggregate macro must reject enums.

#[nexus::aggregate(state = (), error = (), id = ())]
enum NotAStruct {
    A,
    B,
}

fn main() {}
