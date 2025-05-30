use super::User;
use crate::Id;
use crate::command::{aggregate::AggregateRoot, repository::RepositoryError};

pub enum MockUserEventSourceRepsitory<I: Id> {
    LoadReturns(Result<AggregateRoot<User>, RepositoryError<I>>),
    SaveReturns(Result<(), RepositoryError<I>>),
}
