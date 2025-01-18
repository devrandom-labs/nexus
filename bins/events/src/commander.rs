use std::fmt::Debug;
use thiserror::Error as Err;
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot::{self, Sender as OneShotSender},
};
use tokio::task::JoinError;
use tracing::{debug, info, instrument};

#[derive(Debug, Err)]
pub enum Error {
    #[error("{0}")]
    Executor(#[from] JoinError),
}

#[derive(Debug)]
pub struct Command<T>
where
    T: Send + Sync + 'static,
{
    res: OneShotSender<Result<String, Error>>,
    payload: T,
}

impl<T> Command<T>
where
    T: Send + Sync + 'static,
{
    pub fn new(res: OneShotSender<Result<String, Error>>, payload: T) -> Self {
        Command { res, payload }
    }
}

#[derive(Debug, Clone)]
pub struct CommandExecutor<T: Debug + Send + Sync + 'static>(Sender<Command<T>>);

impl<T> CommandExecutor<T>
where
    T: Debug + Send + Sync + 'static,
{
    pub fn new(tx: Sender<Command<T>>) -> Self {
        CommandExecutor(tx)
    }

    #[instrument(skip(self))]
    pub async fn execute(self, command: T) -> Result<String, Error> {
        let (tx, rx) = oneshot::channel();
        let _ = self.0.send(Command::new(tx, command)).await;
        let receive = rx.await;
        let result = receive.unwrap().unwrap();
        info!("response for command 2: {}", result);
        Ok(result)
    }
}

#[instrument]
pub fn commander<T>(bound: usize) -> CommandExecutor<T>
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
    CommandExecutor::new(tx)
}

// this command bus is not tied to any aggregator
