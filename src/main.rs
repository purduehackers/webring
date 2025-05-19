use std::{
    convert::AsRef,
    env::args_os,
    io::{IsTerminal, stderr},
    iter::once,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use clap::{Parser, ValueEnum};
use ftail::Ftail;
use log::{LevelFilter, debug, error, info, warn};
use notify::{EventKind, RecursiveMode, Watcher};
use routes::create_router;
use webring::Webring;

mod checking;
mod homepage;
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

    /// File to read more arguments from
    #[arg(short = 'f', long)]
    arg_file: Option<PathBuf>,

    /// File to read member database from
    #[arg(short = 'm', long)]
    members_file: PathBuf,
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
    // Parse CLI options
    let mut cli = CliOptions::parse();
    if let Some(path) = &cli.arg_file {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let argv0 = args_os().next().unwrap();
                cli.update_from(
                    once(argv0.as_os_str()).chain(contents.split_whitespace().map(AsRef::as_ref)),
                );
            }
            Err(err) => eprintln!("Error: failed to read argument file: {err}"),
        }
    }

    // Set up logging
    let mut logger = Ftail::new();
    if stderr().is_terminal() {
        logger = logger.formatted_console(cli.verbosity.into());
    } else {
        logger = logger.console(cli.verbosity.into());
    }
    if let Some(path) = &cli.log_file {
        logger = logger.single_file(path, true, cli.verbosity.into());
    }
    if let Err(err) = logger.init() {
        eprintln!("Error: failed to initialize logger: {err}");
        return ExitCode::FAILURE;
    }

    // Create webring data structure
    let webring = match Webring::new(cli.members_file.clone(), cli.static_dir.clone()).await {
        Ok(w) => Arc::new(w),
        Err(err) => {
            error!("Failed to create webring: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Create member file watcher
    if let Err(err) = create_webring_reloader(Arc::clone(&webring), &cli.members_file) {
        error!("Unable to watch member file for changes: {err}");
        warn!("Webring will not reload automatically.");
    }
    info!("Watching {} for changes", cli.members_file.display());

    // Start server
    let router = create_router(&cli).with_state(Arc::clone(&webring));
    let bind_addr = &cli.listen_addr;
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

fn create_webring_reloader(webring: Arc<Webring>, members_file: &Path) -> eyre::Result<()> {
    // We need to use a channel to send events, because notify runs the watcher in its own thread
    // which is not part of the tokio runtime. So we can't run `webring.update_from_file()` in the
    // closure because that is async and requires an async runtime. Thus we use the channel to
    // notify an async task that the file should be reloaded.
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let path = members_file.to_owned();
    let mut watcher =
        notify::recommended_watcher(move |maybe_event: notify::Result<notify::Event>| {
            match maybe_event {
                Ok(event) => {
                    debug!("Event observed on {}: {event:#?}", path.display());
                    if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        return;
                    }

                    info!("Detected change to {}. Reloading webring.", path.display());
                    tx.blocking_send(()).unwrap();
                }
                Err(err) => {
                    error!("Error watching file: {err}");
                }
            }
        })?;
    tokio::spawn(async move {
        while let Some(()) = rx.recv().await {
            if let Err(err) = webring.update_from_file().await {
                error!("Failed to update webring: {err}");
            }
            info!("Webring reloaded");
        }
    });
    watcher.watch(members_file, RecursiveMode::NonRecursive)?;
    Box::leak(Box::new(watcher)); // The watcher only works while it exists
    Ok(())
}
