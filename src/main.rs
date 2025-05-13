use std::{
    io::{IsTerminal, stderr},
    process::ExitCode,
};

use axum::{Router, routing::get};
use crashlog::cargo_metadata;
use ftail::Ftail;
use log::{LevelFilter, error, info};

#[tokio::main]
async fn main() -> ExitCode {
    // For crash reporting
    crashlog::setup!(cargo_metadata!(), false);

    // Set up logging
    let level_filter = LevelFilter::Debug;
    let init_result = if stderr().is_terminal() {
        Ftail::new().formatted_console(level_filter).init()
    } else {
        Ftail::new().console(level_filter).init()
    };
    if let Err(err) = init_result {
        eprintln!("Error: failed to initialize logger: {err}");
        return ExitCode::FAILURE;
    }

    // Start server
    let app = Router::new().route("/", get(serve_index));
    let bind_addr = "0.0.0.0:3000";
    match tokio::net::TcpListener::bind(bind_addr).await {
        // Unwrapping this is fine because it will never resolve
        Ok(listener) => {
            info!("Listening on {bind_addr}");
            axum::serve(listener, app).await.unwrap();
        }
        Err(err) => {
            error!("Failed to listen on {bind_addr}: {err}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

async fn serve_index() -> &'static str {
    "Hello, world!\n"
}
