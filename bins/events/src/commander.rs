use std::marker::PhantomData;
use thiserror::Error as ThisError;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, instrument};

#[derive(Debug, ThisError)]
enum Error {
    #[error("there was some error")]
    SomeError,
    #[error("Failed to run commander")]
    Failed,
}
pub trait CommanderResponse {}
pub type Responder<T> = oneshot::Sender<Result<T, Error>>;
pub trait DomainCommand {
    fn get_name(&self) -> &str;
    fn get_version() -> &'static str;
}
pub struct CommandEnvelop<R, C>
where
    R: CommanderResponse,
    C: DomainCommand,
{
    res: Responder<R>,
    payload: C,
}

pub struct Commander<R, C>
where
    R: CommanderResponse,
    C: DomainCommand + Send + Sync,
{
    _responder: PhantomData<R>,
    _payload: PhantomData<C>,
}

impl<R, C> Commander<R, C>
where
    R: CommanderResponse,
    C: DomainCommand + Send + Sync,
{
    #[instrument]
    pub async fn start() -> Result<(), Error> {
        let (tx, mut rx) = mpsc::channel::<CommandEnvelop<R, C>>(15);
        while let Some(c_env) = rx.recv().await {
            // get the handler assigned to this command
            tokio::spawn(async move {
                info!("command name: {}", c_env.payload.get_name());
                // TODO: send stuff back
            });
        }
        Ok(())
    }
}
