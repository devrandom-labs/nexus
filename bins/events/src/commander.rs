use std::fmt::Debug;
use thiserror::Error as Err;
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot::Sender as OneShotSender,
};
use tracing::{debug, info, instrument};

#[derive(Debug, Err)]
pub enum Error {}

#[derive(Debug)]
pub struct Command<T>
where
    T: Debug + Send + Sync + 'static,
{
    res: OneShotSender<Result<String, Error>>,
    payload: T,
}

impl<T> Command<T>
where
    T: Debug + Send + Sync + 'static,
{
    pub fn new(res: OneShotSender<Result<String, Error>>, payload: T) -> Self {
        Command { res, payload }
    }
}

#[instrument]
pub fn commander<T>(bound: usize) -> Sender<Command<T>>
where
    T: Debug + Send + Sync + 'static,
{
    debug!("initializing channel between commander and the application");
    let (tx, mut rx) = mpsc::channel::<Command<T>>(bound);
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            info!("received in command bus: {:?}", command.payload);
            let _ = command.res.send(Ok(String::from("got your message guv")));
        }
    });
    tx
}
