pub trait Aggregate {
    type Root;
    type Events;
    type Error: std::error::Error;
    fn apply(state: Option<Self::Root>, events: &Self::Events) -> Result<Self::Root, Self::Error>;
}
