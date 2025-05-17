//! Routes and endpoints

use axum::{
    extract::State, response::{Html, IntoResponse, NoContent}, routing::get, Router
};
use reqwest::StatusCode;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::{CliOptions, webring::Webring};

/// Creates a [`Router`] with the routes for our application.
pub fn create_router(cli: &CliOptions) -> Router<&'static Webring> {
    Router::new()
        .route("/", get(serve_index))
        .nest_service("/static", ServeDir::new(&cli.static_dir))
        .route(
            "/healthcheck",
            get(async || NoContent).layer(CorsLayer::new().allow_origin(Any)),
        )
}

async fn serve_index(State(webring): State<&Webring>) -> Result<Html<String>, HomepageError> {
    Ok(Html(webring.homepage().await?.to_html().to_owned()))
}

#[derive(Debug)]
struct HomepageError(eyre::Report);
impl From<eyre::Report> for HomepageError {
    fn from(value: eyre::Report) -> Self {
        Self(value)
    }
}
impl IntoResponse for HomepageError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error rendering home page: {}", self.0),
        )
            .into_response()
    }
}
