use std::{
    convert::AsRef,
    env::args_os,
    io::{IsTerminal, stderr},
    iter::once,
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use axum::http::{Uri, uri::Scheme};
use clap::{Parser, ValueEnum};
use discord::DiscordNotifier;
use ftail::Ftail;
use log::LevelFilter;
use reqwest::Url;
use routes::create_router;
use sarlacc::Intern;
use webring::Webring;

mod checking;
mod discord;
mod homepage;
mod routes;
mod stats;
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

    #[arg(short = 'a', long, default_value = "https://ring.purduehackers.com", value_parser = parse_uri)]
    address: Intern<Uri>,

    /// Discord webhook URL
    #[arg(long)]
    discord_webhook_url: Option<Url>,
}

fn parse_uri(str: &str) -> eyre::Result<Intern<Uri>> {
    let uri = str.parse::<Uri>()?;

    if !uri.path().trim_matches('/').is_empty() {
        return Err(eyre::eyre!(
            "Expected the address URI to not have a path component."
        ));
    }

    if ![None, Some(&Scheme::HTTPS), Some(&Scheme::HTTP)].contains(&uri.scheme()) {
        return Err(eyre::eyre!(
            "Expected the scheme to be either `HTTP` or `HTTPS`"
        ));
    }

    if uri.authority().is_none() {
        return Err(eyre::eyre!(
            "Expected the address URI to have an authority component (to not be a relative path)."
        ));
    }

    Ok(Intern::new(uri))
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

    // Create Discord notifier
    let maybe_notifier = cli.discord_webhook_url.as_ref().map(DiscordNotifier::new);

    // Create webring data structure
    let webring = match Webring::new(
        cli.members_file.clone(),
        cli.static_dir.clone(),
        cli.address,
        maybe_notifier,
    )
    .await
    {
        Ok(w) => Arc::new(w),
        Err(err) => {
            log::error!("Failed to create webring: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Perform site checks every 5 minutes
    {
        const SITE_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 5);
        let webring_for_task = Arc::clone(&webring);
        tokio::spawn(async move {
            loop {
                webring_for_task.check_members().await;
                tokio::time::sleep(SITE_CHECK_INTERVAL).await;
            }
        });
    }

    // Create member file watcher
    if let Err(err) = webring.enable_reloading() {
        log::error!("Unable to watch member file for changes: {err}");
        log::warn!("Webring will not reload automatically.");
    }
    webring.enable_ip_pruning(chrono::Duration::hours(1));
    log::info!("Watching {} for changes", cli.members_file.display());

    // Start server
    let router = create_router(&cli.static_dir)
        .with_state(Arc::clone(&webring))
        .into_make_service_with_connect_info::<SocketAddr>();
    let bind_addr = &cli.listen_addr;
    match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(listener) => {
            match listener.local_addr() {
                Ok(addr) => log::info!("Listening on http://{addr}"),
                Err(err) => {
                    log::info!("Listening...");
                    log::warn!("Failed to get the address we're listening on: {err}");
                }
            }
            // Unwrapping this is fine because it will never resolve
            axum::serve(listener, router).await.unwrap();
        }
        Err(err) => {
            log::error!("Failed to listen on {bind_addr}: {err}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
