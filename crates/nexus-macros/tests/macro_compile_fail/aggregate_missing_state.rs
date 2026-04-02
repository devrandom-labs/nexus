/// aggregate macro must require `state`.

#[nexus::aggregate(error = std::io::Error, id = u64)]
struct MissingState;

fn main() {}
