use super::User;
use crate::command::{
    aggregate::{AggregateRoot, AggregateType},
    repository::{EventSourceRepository, RepositoryError},
};
use crate::{DomainEvent, Id};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub enum MockUserEventSourceRepsitory<I: Id> {
    LoadReturns(Result<AggregateRoot<User>, RepositoryError<I>>),
    SaveReturns(Result<(), RepositoryError<I>>),
}
