use pingora::server::configuration::Opt;
use pingora::server::Server;
use tracing::{debug, error, info, instrument};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter, Layer,
};

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

    // -------------------- load config -------------------- //
    // life time of this gateway is based on the config.
    // should I allow to change config on runtime and reload it?
    let mut opt = Opt::parse_args();
    opt.conf = opt
        .conf
        .or(Some(String::from("/etc/steersman/config.yaml")));
    debug!("{:?}", opt.conf);
    let mut server = Server::new(Some(opt))
        .inspect_err(|err| error!(?err))
        .unwrap();

    server.bootstrap();
}

// Phase 1
// TODO: create a basic gateway and add that gateway service to server
// TODO: add tracing to that gateway
