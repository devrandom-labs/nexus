use tokio::sync::{
    mpsc::{self, Sender},
    oneshot::Sender as OneShotSender,
};
use tracing::{debug, info, instrument};

#[derive(Debug)]
pub struct Command {
    res: OneShotSender<Result<String, String>>,
    payload: String,
}

impl Command {
    pub fn new(res: OneShotSender<Result<String, String>>, payload: String) -> Self {
        Command { res, payload }
    }
}

#[instrument]
pub fn commander(bound: usize) -> Sender<Command> {
    debug!("initializing channel between commander and the application");
    let (tx, mut rx) = mpsc::channel::<Command>(bound);
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            info!("received in command bus: {}", command.payload);
            let _ = command.res.send(Ok(String::from("got your message guv")));
        }
    });
    tx
}
