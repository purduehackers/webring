//! Routes and endpoints

use axum::{Router, response::NoContent, routing::get};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};

use crate::CliOptions;

/// Creates a [`Router`] with the routes for our application.
pub fn create_router(cli: &CliOptions) -> Router {
    Router::new()
        .route_service("/", ServeFile::new(cli.static_dir.join("index.html")))
        .nest_service("/static", ServeDir::new(&cli.static_dir))
        .route(
            "/healthcheck",
            get(async || NoContent).layer(CorsLayer::new().allow_origin(Any)),
        )
}
