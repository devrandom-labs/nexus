use thiserror::Error as Err;
pub mod message_handler;

#[derive(Err, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Error {}
