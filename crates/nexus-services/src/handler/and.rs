#![allow(dead_code)]
use super::Handler;
use pin_project::pin_project;

#[derive(Clone, Copy, Debug)]
pub struct And<T, U> {
    pub(crate) first: T,
    pub(crate) second: U,
}

#[pin_project(project = StateProj)]
enum State<T, TE, U: Handler> {
    First(#[pin] T, U),
    Second(Option<TE>, #[pin] U::Future),
    Done,
}
