//! Routes and endpoints

use std::{collections::HashMap, str::FromStr};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, Uri},
    response::{Html, IntoResponse, NoContent, Redirect, Response},
    routing::get,
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
        .nest_service("/static", ServeDir::new(&cli.static_dir))
        .route(
            "/healthcheck",
            get(async || NoContent).layer(CorsLayer::new().allow_origin(Any)),
        )
        .route("/", get(serve_index))
        .route("/next", get(serve_next))
        // Support /prev and /previous as aliases of each other
        .route("/prev", get(serve_previous))
        .route("/previous", get(serve_previous))
        .route("/random", get(serve_random))
}

async fn serve_index(State(webring): State<&Webring>) -> Result<Response, HomepageError> {
    Ok(Html(webring.homepage().await?.to_html().to_owned()).into_response())
}

/// Serve the `/next` endpoint.
///
/// For each request, this function:
/// 1. Gets the origin using [`get_origin_from_request()`].
/// 2. Finds the next site in the [`Webring`].
/// 3. Redirects to the site found.
///
/// If finding the next site fails, an HTTP 502 (Service Unavailable) response is sent.
async fn serve_next(
    State(webring): State<&Webring>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Redirect, Response> {
    let origin = get_origin_from_request(&headers, &params)?;
    // FIXME: Differentiate between origin not found and all sites failing checks
    match webring.next_page(&origin).ok() {
        Some(page) => Ok(Redirect::to(page.to_string().as_str())),
        None => Err((StatusCode::SERVICE_UNAVAILABLE, "No suitable sites found").into_response()),
    }
}

/// Serve the `/previous` and `/prev` endpoints.
///
/// For each request, this function:
/// 1. Gets the origin using [`get_origin_from_request()`].
/// 2. Finds the previous site in the [`Webring`].
/// 3. Redirects to the site found.
///
/// If finding the previous site fails, an HTTP 502 (Service Unavailable) response is sent.
async fn serve_previous(
    State(webring): State<&Webring>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Redirect, Response> {
    let origin = get_origin_from_request(&headers, &params)?;
    // FIXME: Differentiate between origin not found and all sites failing checks
    match webring.prev_page(&origin).ok() {
        Some(page) => Ok(Redirect::to(page.to_string().as_str())),
        None => Err((StatusCode::SERVICE_UNAVAILABLE, "No suitable sites found").into_response()),
    }
}

/// Serve the `/random` endpoint.
///
/// For each request, this function:
/// 1. Gets the origin using [`get_origin_from_request()`], ignoring errors.
/// 2. Finds a random site in the [`Webring`] not matching the origin (if present).
/// 3. Redirects to the site found.
///
/// If finding a random site fails, an HTTP 502 (Service Unavailable) response is sent.
async fn serve_random(
    State(webring): State<&Webring>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Redirect, Response> {
    let maybe_origin = get_origin_from_request(&headers, &params).ok();
    match webring.random_page(maybe_origin.as_ref()) {
        Some(page) => Ok(Redirect::to(page.to_string().as_str())),
        None => Err((StatusCode::SERVICE_UNAVAILABLE, "No suitable sites found").into_response()),
    }
}

/// Get a URI representing the origin of the request.
///
/// Uses the `?host=` query parameter if present, or else uses the `Referer` request header.
/// If a suitable origin isn't found (e.g. neither the parameter nor header are present) or parsing
/// the found URI fails, an `Err` is returned with a [`Response`] suitable for returning to the
/// client.
fn get_origin_from_request(
    headers: &HeaderMap,
    params: &HashMap<String, String>,
) -> Result<Uri, Response> {
    match params.get("host") {
        Some(host) => match Uri::from_str(host) {
            Ok(uri) => Ok(uri),
            Err(err) => Err((
                StatusCode::BAD_REQUEST,
                format!("host query parameter contains invalid URI: {err}"),
            )
                .into_response()),
        },
        None => match headers.get("referer") {
            Some(referer) => match referer.to_str() {
                Ok(referer_str) => match Uri::from_str(referer_str) {
                    Ok(uri) => Ok(uri),
                    Err(err) => Err((
                        StatusCode::BAD_REQUEST,
                        format!("Invalid Referer URI: {err}"),
                    )
                        .into_response()),
                },
                Err(err) => Err((
                    StatusCode::BAD_REQUEST,
                    format!("Error parsing Referer URI: {err}"),
                )
                    .into_response()),
            },
            None => Err((
                StatusCode::BAD_REQUEST,
                "Missing Referer header and ?host= parameter",
            )
                .into_response()),
        },
    }
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
