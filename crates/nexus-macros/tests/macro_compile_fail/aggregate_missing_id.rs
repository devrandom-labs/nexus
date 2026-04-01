/// aggregate macro must require `id`.

#[nexus::aggregate(state = (), error = std::io::Error)]
struct MissingId;

fn main() {}
