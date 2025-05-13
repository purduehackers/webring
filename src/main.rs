use std::{
    io::{IsTerminal, stderr},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, ValueEnum};
use crashlog::cargo_metadata;
use ftail::Ftail;
use log::{LevelFilter, error, info};
use routes::create_router;

mod render_homepage;
mod routes;
mod webring;

/// Default log level.
const DEFAULT_LOG_LEVEL: LevelFilterWrapper = if cfg!(debug_assertions) {
    LevelFilterWrapper::Debug
} else {
    LevelFilterWrapper::Info
};

#[derive(clap::Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
struct CliOptions {
    /// Address/port to listen on
    #[arg(short, long = "listen-on", default_value = "0.0.0.0:3000")]
    listen_addr: SocketAddr,

    /// Log verbosity
    #[arg(short, long, alias = "log-level", value_enum, default_value_t = DEFAULT_LOG_LEVEL)]
    verbosity: LevelFilterWrapper,

    /// File to print logs to in addition to the console
    #[arg(short = 'o', long)]
    log_file: Option<PathBuf>,

    /// Directory from which to serve static content
    #[arg(short = 'd', long, default_value = "static")]
    static_dir: PathBuf,
}

// This type exists so clap can figure out what variants are available for the verbosity option.
// If we use LevelFilter directly, it uses the Display and FromStr implementations, which means
// there isn't a list of possible variants for clap to use.
#[derive(ValueEnum, Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LevelFilterWrapper {
    Off,
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LevelFilterWrapper> for LevelFilter {
    fn from(val: LevelFilterWrapper) -> Self {
        match val {
            LevelFilterWrapper::Off => LevelFilter::Off,
            LevelFilterWrapper::Trace => LevelFilter::Trace,
            LevelFilterWrapper::Debug => LevelFilter::Debug,
            LevelFilterWrapper::Info => LevelFilter::Info,
            LevelFilterWrapper::Warn => LevelFilter::Warn,
            LevelFilterWrapper::Error => LevelFilter::Error,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // For crash reporting
    crashlog::setup!(cargo_metadata!(), false);

    let cli = CliOptions::parse();

    // Set up logging
    let mut logger = Ftail::new();
    if stderr().is_terminal() {
        logger = logger.formatted_console(cli.verbosity.into());
    } else {
        logger = logger.console(cli.verbosity.into());
    }
    if let Some(path) = &cli.log_file {
        // TODO: Remove if/when https://github.com/tjardoo/ftail/pull/9 makes it into a release
        let Some(path_str) = path.to_str() else {
            eprintln!("Error: log file path must be valid UTF-8");
            return ExitCode::FAILURE;
        };
        logger = logger.single_file(path_str, true, cli.verbosity.into());
    }
    if let Err(err) = logger.init() {
        eprintln!("Error: failed to initialize logger: {err}");
        return ExitCode::FAILURE;
    }

    // Start server
    let router = create_router(&cli);
    let bind_addr = "0.0.0.0:3000";
    match tokio::net::TcpListener::bind(bind_addr).await {
        // Unwrapping this is fine because it will never resolve
        Ok(listener) => {
            info!("Listening on http://{bind_addr}");
            axum::serve(listener, router).await.unwrap();
        }
        Err(err) => {
            error!("Failed to listen on {bind_addr}: {err}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
