use chrono::{DateTime, Utc};
use std::{
    fmt::{Debug, Display},
    ops::Deref,
};
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(Ulid);

impl CommandId {
    pub fn new() -> Self {
        let id = Ulid::new();
        Self(id)
    }

    pub fn id(&self) -> &Ulid {
        &self.0
    }
}

impl Deref for CommandId {
    type Target = Ulid;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for CommandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command_id: {}", &self.0)
    }
}

impl Default for CommandId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Command<P>
where
    P: Debug,
{
    id: CommandId,
    created_at: DateTime<Utc>,
    payload: P,
}

impl<P> Command<P>
where
    P: Debug,
{
    pub fn new(payload: P) -> Self {
        Self {
            id: CommandId::default(),
            created_at: Utc::now(),
            payload,
        }
    }

    pub fn id(&self) -> &CommandId {
        &self.id
    }

    pub fn payload(&self) -> &P {
        &self.payload
    }

    pub fn created_at(&self) -> &DateTime<Utc> {
        &self.created_at
    }
}
