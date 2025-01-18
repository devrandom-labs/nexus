use tokio::sync::mpsc::{self, Sender};
use tracing::{debug, info, instrument};

#[instrument]
pub fn commander(bound: usize) -> Sender<String> {
    debug!("initializing channel between commander and the application");
    let (tx, mut rx) = mpsc::channel::<String>(bound);
    tokio::spawn(async move {
        while let Some(rec) = rx.recv().await {
            info!("received: {}", rec);
        }
    });
    tx
}
