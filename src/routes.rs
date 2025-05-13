//! Routes and endpoints

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use crate::CliOptions;

/// Creates a [`Router`] with the routes for our application.
pub fn create_router(cli: &CliOptions) -> Router {
    Router::new()
        .route_service("/", ServeFile::new(cli.static_dir.join("index.html")))
        .nest_service("/static", ServeDir::new(&cli.static_dir))
}
