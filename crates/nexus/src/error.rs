use thiserror::Error as Err;

#[derive(Debug, Err)]
pub enum Error {
    #[error("Store Error")]
    Store,
}
