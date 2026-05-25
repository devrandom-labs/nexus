use nexus_macros::transforms;

struct Order;

#[derive(Debug)]
struct MyError;
impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error")
    }
}
impl std::error::Error for MyError {}

// Declares transforms 1→2 and 4→5 with no 2→3 or 3→4 step. The macro must
// reject this because version 3's `from` step is missing — the chain has a
// gap and rehydrating a v3 event would silently leave it at v3 while
// current_version is 5.
#[transforms(aggregate = Order, error = MyError)]
impl OrderTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }

    #[transform(event = "OrderCreated", from = 4, to = 5)]
    fn v4_to_v5(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }
}

fn main() {}
