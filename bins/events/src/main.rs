use tokio::sync::oneshot;
use tracing::{info, instrument};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter, Layer,
};

pub mod commander;
pub mod domain;
pub mod store;

#[instrument]
#[tokio::main]
async fn main() {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(console).init();

    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");

    info!("🚀🚀🎆{}:{}@{}🎆🚀🚀", workspace, name, version);

    let sender = commander::commander(20);
    let command_handler_1 = tokio::spawn({
        let sender = sender.clone();
        async move {
            let (tx, rx) = oneshot::channel();
            let _ = sender
                .send(commander::Command::new(tx, String::from("create event")))
                .await;

            let receive = rx.await;
            info!("response for command 1: {}", receive.unwrap().unwrap());
        }
    });

    let command_handler_2 = tokio::spawn({
        let sender = sender.clone();
        async move {
            let (tx, rx) = oneshot::channel();
            let _ = sender
                .send(commander::Command::new(tx, String::from("delete event")))
                .await;
            let receive = rx.await;
            info!("response for command 2: {}", receive.unwrap().unwrap());
        }
    });

    let _ = tokio::join!(command_handler_1, command_handler_2);
}
