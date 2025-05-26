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
use ftail::Ftail;
use routes::create_router;
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
    let mut logger = Ftail::new();
    if stderr().is_terminal() {
        logger = logger.formatted_console(cfg.logging.verbosity);
    } else {
        logger = logger.console(cfg.logging.verbosity);
    }
    if let Some(path) = &cfg.logging.log_file {
        logger = logger.single_file(path, true, cfg.logging.verbosity);
    }
    if let Err(err) = logger.init() {
        eprintln!("Error: failed to initialize logger: {err}");
        return ExitCode::FAILURE;
    }

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

    webring.enable_ip_pruning(chrono::Duration::hours(1));
    if let Err(err) = webring.enable_reloading(&cli.config_file) {
        log::error!("Failed to watch configuration files for changes: {err}");
        log::warn!("The webring will not be reloaded when files change.");
    }

    // Start server
    let router = create_router(&cfg.webring.static_dir)
        .with_state(Arc::clone(&webring))
        .into_make_service_with_connect_info::<SocketAddr>();
    let bind_addr = &cfg.network.listen_addr;
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
