mod bus;
mod service;
mod store;

pub struct Cqrs;

// should take event store repository
// takes command handlers
// maps commands to command handlers how?

#[cfg(test)]
mod test {
    use super::*;
    pub fn should_take_event_store() {}
    pub fn should_take_command_handlers() {}
}
