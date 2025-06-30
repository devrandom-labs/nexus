use nexus_macros::Message;

fn main() {
    #[allow(dead_code)]
    #[derive(Message)]
    struct SomeMessage {
        user_id: String,
    }
}
