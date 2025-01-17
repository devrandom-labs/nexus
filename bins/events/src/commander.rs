use std::fmt::Debug;
use std::marker::PhantomData;
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    oneshot,
};
use tracing::{info, instrument};

mod error {
    use thiserror::Error as ThisError;
    #[derive(Debug, ThisError)]
    pub enum Error {
        #[error("there was some error")]
        SomeError,
        #[error("Failed to run commander")]
        Failed,
    }
}

// every command should implement this
pub trait DomainCommand {
    fn get_name(&self) -> &str;
    fn get_version() -> &'static str;
}

pub trait Response {}

pub type Responder<T> = oneshot::Sender<Result<T, error::Error>>;

// internal envelope for every command and passed around in the system.
pub struct CommandEnvelop<R, C>
where
    R: Response,
    C: DomainCommand,
{
    res: Responder<R>,
    payload: C,
}
// -------------------- main work -------------------- //
//

#[derive(Debug)]
pub struct Bus<R, C>
where
    R: Response + Debug,
    C: DomainCommand + Send + Sync + 'static + Debug,
{
    receiver: Receiver<CommandEnvelop<R, C>>,
}

impl<R, C> Bus<R, C>
where
    R: Response + Debug,
    C: DomainCommand + Send + Sync + 'static + Debug,
{
    #[instrument]
    pub async fn start(mut self) -> Result<(), error::Error> {
        while let Some(c_env) = self.receiver.recv().await {
            // get the handler assigned to this command
            tokio::spawn(async move {
                info!("command name: {}", c_env.payload.get_name());
                // TODO: send stuff back
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Commander<R, C>
where
    R: Response,
    C: DomainCommand + Send + Sync + 'static,
{
    _res: PhantomData<R>,
    _com: PhantomData<C>,
    bound: usize,
}

impl<R, C> Commander<R, C>
where
    R: Response,
    C: DomainCommand + Send + Sync,
{
    pub fn new(bound: usize) -> Self {
        // let (tx, mut rx) = mpsc::channel::<CommandEnvelop<R, C>>(bound);
        Commander { bound }
    }
}
