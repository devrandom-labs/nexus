use thiserror::Error as Err;

#[derive(Debug, Err)]
pub enum Error {
    #[error("Could not create dedup id")]
    CouldNotCreateDedupId,
}
