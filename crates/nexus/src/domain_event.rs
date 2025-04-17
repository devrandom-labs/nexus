use std::fmt::{Debug, Display};

pub trait DomainEvent: Debug + Display {
    fn get_version() -> &'static str;
}
