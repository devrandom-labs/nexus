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
// TODO: supply config.yaml file and check if you can see
// TODO: set the config to pingora server successfully.
// TODO: set tracing in pingora server
// TODO: clean up the architectrue and maybe impl some design patterns for config or clap.
// TODO: write tests to ensure config to server capabilities and failure
// TODO: ensure tracing and metrics
// TODO: have prometheus or jaeger (depending) and get open telemetry meterics to it.

// steps
// get pingora::core::configuration::Opt and check if configuration file has been specified.
