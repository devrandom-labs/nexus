pub trait Aggregate {
    type Error: std::error::Error;
    type Commands;
    type Events;
}
