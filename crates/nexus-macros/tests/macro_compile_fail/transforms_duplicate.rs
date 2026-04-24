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

#[transforms(aggregate = Order, error = MyError)]
impl OrderTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2_a(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }

    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2_b(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }
}

fn main() {}
