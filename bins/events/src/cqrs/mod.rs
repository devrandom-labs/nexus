pub trait Aggregate {
    type Commands;
    type Events;
    type Errors;
    fn handle(&self, commands: Self::Commands) -> Result<Self::Events, Self::Errors>;
}
