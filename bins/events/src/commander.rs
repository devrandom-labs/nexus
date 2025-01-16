trait CommandDispatcher {
    type Commands;
    type Error;
    fn dispatch(command: Self::Commands) -> Result<(), Self::Error>;
}

trait Commandable {} //  tokio service

trait CommandHandler {
    type Dispatcher: CommandDispatcher;
    type Commands: Commandable;
    fn handle(commands: &[Self::Commands]) -> Self;
    fn run(self) -> Result<Self::Dispatcher, String>;
}
