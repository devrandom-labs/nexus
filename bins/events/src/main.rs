use tokio::sync::{mpsc, oneshot};
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

    let event = domain::event::Event::default();
    info!(?event);

    // created a channel which takes commands enum
    // configure the bounds of this channel for better control.

    tokio::spawn({
        let tx = tx.clone();
        async move {
            tx.send(Command::Create {
                title: "some festival".to_string(),
            })
            .await
            .unwrap();
        }
    });
    tokio::spawn({
        let tx = tx.clone();
        async move {
            tx.send(Command::Create {
                title: "other festival".to_string(),
            })
            .await
            .unwrap();
        }
    });
}

#[derive(Debug)]
pub enum Command {
    Create { title: String },
}
impl commander::DomainCommand for Command {
    fn get_name(&self) -> &str {
        match self {
            Command::Create { .. } => "create events",
        }
    }
    fn get_version() -> &'static str {
        "1"
    }
}
