pub trait Aggregate {
    type Commands;
    type Events;
    type Errors;
    fn get_aggregate() -> String;
    fn handle(&self, commands: Self::Commands) -> Result<Vec<Self::Events>, Self::Errors>;
    fn apply(&mut self, event: Self::Events);
}
