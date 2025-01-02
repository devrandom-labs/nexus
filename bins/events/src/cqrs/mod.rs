pub trait Aggregate: Send + Sync {
    type Commands;
    type Events;
    type Errors;
    fn handle(&self, commands: Self::Commands) -> Result<Vec<Self::Events>, Self::Errors>;
}
