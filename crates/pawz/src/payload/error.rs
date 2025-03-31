use thiserror::Error as TError;

#[derive(Debug, TError)]
pub enum Error {
    #[error("Reply has no type schema")]
    NoTypeSchema,
    #[error("Empty error message")]
    EmptyMessage,
}
