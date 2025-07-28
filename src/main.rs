//! # Purdue Hackers Webring
//!
//! Implements a web server that serves the Purdue Hackers Webring.

use std::{
    io::{IsTerminal, stderr},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use clap::Parser;
use config::Config;
use routes::create_router;
use sarlacc::num_objects_interned;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;
use webring::Webring;

mod checking;
mod config;
mod discord;
mod homepage;
mod routes;
mod stats;
mod webring;

#[derive(clap::Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
struct CliOptions {
    /// Path of configuration file to read
    #[arg(short = 'f', default_value = "webring.toml")]
    config_file: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Parse CLI options
    let cli = CliOptions::parse();

    // Load config
    let cfg = match Config::parse_from_file(&cli.config_file).await {
        Ok(cfg) => Arc::new(cfg),
        Err(err) => {
            eprintln!("Failed to read configuration file: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Set up logging

    // Create two loggers, only one of which will be used depending on whether
    // stderr is a terminal. This is needed since they have different types, so they must be
    // different variables.
    let (pretty, regular) = if stderr().is_terminal() {
        (Some(tracing_subscriber::fmt::layer().pretty()), None)
    } else {
        (None, Some(tracing_subscriber::fmt::layer()))
    };
    // If a log file is set, create a layer that writes to it.
    let (file_logger, _guard) = if let Some(path) = &cfg.logging.log_file {
        let (Some(dir), Some(filename)) = (path.parent(), path.file_name()) else {
            error!("Log file must be a valid file path");
            return ExitCode::FAILURE;
        };
        let file = tracing_appender::rolling::never(dir, filename);
        let (logger, guard) = tracing_appender::non_blocking(file);
        (Some(logger), Some(guard))
    } else {
        (None, None)
    };
    let file_layer = file_logger.map(|logger| {
        tracing_subscriber::fmt::layer()
            .with_writer(logger)
            .with_ansi(false)
    });
    // Initialize the tracing subscriber with the appropriate layers and the level filter
    tracing_subscriber::registry()
        .with(pretty)
        .with(regular)
        .with(file_layer)
        .with(cfg.logging.verbosity)
        .init();

    // Create webring data structure
    let webring = Arc::new(Webring::new(&cfg));

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

    // Send Discord notifications every 24 hours
    if let Some(notifier) = webring.notifier() {
        const DISCORD_NOTIFICATION_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
        let notifier = Arc::clone(notifier);
        tokio::spawn(async move {
            loop {
                let (sent, failed) = notifier.dispatch_messages().await;
                if sent > 0 || failed > 0 {
                    info!(sent, failed, "Discord notifications dispatched");
                }
                tokio::time::sleep(DISCORD_NOTIFICATION_INTERVAL).await;
            }
        });
    }

    webring.enable_ip_pruning(chrono::Duration::hours(1));
    if let Err(err) = webring.enable_reloading(&cli.config_file) {
        error!(%err, "Failed to watch configuration files for changes");
        warn!("The webring will not be reloaded when files change.");
    }

    tokio::spawn(async {
        info!("{} objects are interned", num_objects_interned());
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    });

    // Start server
    let router = create_router(&cfg.webring.static_dir)
        .with_state(Arc::clone(&webring))
        .into_make_service_with_connect_info::<SocketAddr>();
    let bind_addr = &cfg.network.listen_addr;
    match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(listener) => {
            match listener.local_addr() {
                Ok(addr) => info!(%addr, "Listening..."),
                Err(err) => {
                    info!("Listening...");
                    warn!(%err, "Failed to get the address we're listening on");
                }
            }
            // Unwrapping this is fine because it will never resolve
            axum::serve(listener, router).await.unwrap();
        }
        Err(err) => {
            error!(addr = %bind_addr, %err, "Failed to listen");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
