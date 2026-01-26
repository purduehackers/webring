/*
Copyright (C) 2025 Kian Kasad and Henry Rovnyak

This file is part of the Purdue Hackers webring.

The Purdue Hackers webring is free software: you can redistribute it and/or
modify it under the terms of the GNU Affero General Public License as
published by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

The Purdue Hackers webring is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License
for more details.

You should have received a copy of the GNU Affero General Public License along
with the Purdue Hackers webring. If not, see <https://www.gnu.org/licenses/>.
*/

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
use tracing::{debug, debug_span, error, info, instrument, warn};
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

fn main() -> ExitCode {
    // Parse CLI options
    let cli = CliOptions::parse();

    // Load config
    let maybe_cfg = {
        let cfg_toml = match std::fs::read_to_string(&cli.config_file) {
            Ok(contents) => contents,
            Err(err) => {
                eprintln!(
                    "Failed to read configuration file {}: {}",
                    cli.config_file.display(),
                    err
                );
                return ExitCode::FAILURE;
            }
        };
        Config::parse_from_toml(&cfg_toml).map(Arc::new)
    };
    let cfg = match maybe_cfg {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!(
                "Failed to parse configuration file {}: {}",
                cli.config_file.display(),
                err
            );
            return ExitCode::FAILURE;
        }
    };

    // Set up Sentry integration. This must be done before the Tokio runtime is
    // created, according to Sentry's docs.
    let _guard = sentry::init((
        cfg.logging.sentry_dsn.as_ref(),
        sentry::ClientOptions {
            release: sentry::release_name!(),
            send_default_pii: true,
            ..Default::default()
        },
    ));

    // Construct async runtime and run async entry point
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async_main(cli, cfg))
}

/// Async entry point to the webring
#[instrument]
async fn async_main(cli: CliOptions, cfg: Arc<Config>) -> ExitCode {
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

    webring.enable_ip_pruning(chrono::Duration::hours(1));
    if let Err(err) = webring.enable_reloading(&cli.config_file) {
        error!(%err, "Failed to watch configuration files for changes");
        warn!("The webring will not be reloaded when files change.");
    }

    tokio::spawn(async {
        loop {
            debug_span!("monitor interned objects").in_scope(|| {
                debug!(count = num_objects_interned(), "interned object count");
            });
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
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
