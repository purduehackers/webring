//! Routes and endpoints

use std::{
    collections::HashMap, error::Error, fmt::Display, fmt::Write, str::FromStr, sync::LazyLock,
};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, Uri, uri::InvalidUri},
    response::{Html, IntoResponse, NoContent, Redirect, Response},
    routing::get,
};
use reqwest::StatusCode;
use tera::Tera;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::{
    CliOptions,
    webring::{TraverseWebringError, Webring},
};

static ERROR_TEMPLATE: LazyLock<Tera> = LazyLock::new(create_error_template);

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

fn create_error_template() -> Tera {
    let template_src = include_str!("templates/error.html");
    let mut tera = Tera::default();
    tera.add_raw_template("error.html", template_src).unwrap();
    tera
}

async fn serve_index(State(webring): State<&Webring>) -> Result<Response, RouteError> {
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
) -> Result<Redirect, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.next_page(&origin)?;
    Ok(Redirect::to(page.to_string().as_str()))
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
) -> Result<Redirect, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.prev_page(&origin)?;
    Ok(Redirect::to(page.to_string().as_str()))
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
) -> Result<Redirect, RouteError> {
    let maybe_origin = get_origin_from_request(headers, params).ok();
    let page = webring.random_page(maybe_origin.as_ref())?;
    Ok(Redirect::to(page.to_string().as_str()))
}

/// Get a URI representing the origin of the request.
///
/// Uses the `?host=` query parameter if present, or else uses the `Referer` request header.
/// If a suitable origin isn't found (e.g. neither the parameter nor header are present) or parsing
/// the found URI fails, an `Err` is returned with a [`Response`] suitable for returning to the
/// client.
fn get_origin_from_request(
    mut headers: HeaderMap,
    mut params: HashMap<String, String>,
) -> Result<Uri, RouteError> {
    match params.remove("host") {
        Some(host) => match Uri::from_str(&host) {
            Ok(uri) => Ok(uri),
            Err(err) => Err(RouteError::InvalidOriginURI {
                uri: host,
                reason: err,
                place: OriginUriLocation::HostQueryParam,
            }),
        },
        None => match headers.remove("referer") {
            Some(referer) => match referer.to_str() {
                Ok(referer_str) => match Uri::from_str(referer_str) {
                    Ok(uri) => Ok(uri),
                    Err(err) => Err(RouteError::InvalidOriginURI {
                        uri: referer_str.to_owned(),
                        reason: err,
                        place: OriginUriLocation::RefererHeader,
                    }),
                },
                Err(_) => Err(RouteError::HeaderToStr(OriginUriLocation::RefererHeader)),
            },
            None => Err(RouteError::MissingOriginURI),
        },
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum OriginUriLocation {
    RefererHeader,
    HostQueryParam,
}

impl Display for OriginUriLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OriginUriLocation::RefererHeader => "Referer header",
            OriginUriLocation::HostQueryParam => "?host= query parameter",
        }
        .fmt(f)
    }
}

#[derive(Debug, thiserror::Error)]
enum RouteError {
    #[error(r#"Origin URI "{uri}" found in {place} is invalid: {reason}"#)]
    InvalidOriginURI {
        uri: String,
        #[source]
        reason: InvalidUri,
        place: OriginUriLocation,
    },

    #[error("Request doesn't indicate which site it originates from.")]
    MissingOriginURI,

    #[error("{0} contains data which is not valid UTF-8")]
    HeaderToStr(OriginUriLocation),

    #[error("Error traversing webring: {0}")]
    Traversal(
        #[from]
        #[source]
        TraverseWebringError,
    ),

    #[error("Error rendering homepage: {0}")]
    RenderHomepage(
        #[from]
        #[source]
        eyre::Report,
    ),
}

impl IntoResponse for RouteError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            RouteError::Traversal(TraverseWebringError::AllMembersFailing) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            RouteError::Traversal(
                TraverseWebringError::NoAuthority(_) | TraverseWebringError::AuthorityNotFound(_),
            )
            | RouteError::InvalidOriginURI { .. }
            | RouteError::MissingOriginURI
            | RouteError::HeaderToStr(_) => StatusCode::BAD_REQUEST,
            RouteError::RenderHomepage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let mut context = tera::Context::new();
        context.insert("status_code", &status.as_u16());
        context.insert(
            "status_text",
            status.canonical_reason().unwrap_or("Unknown Error"),
        );
        context.insert(
            "description",
            match status {
                StatusCode::BAD_REQUEST => "The request we received wasn't quite right.",
                StatusCode::SERVICE_UNAVAILABLE => "We couldn't find a site to route you to.",
                StatusCode::INTERNAL_SERVER_ERROR => "Something unexpected went wrong.",
                _ => "We're not quite sure.",
            },
        );

        // Render error and chain
        let mut error_explanation = format!("{}", &self);
        let mut source = self.source();
        while let Some(error) = source {
            write!(&mut error_explanation, "\nCaused by: {error}").unwrap();
            source = error.source();
        }
        context.insert("error", &error_explanation);

        // We can unwrap this because the template is compiled into the program, so we know it'll
        // render properly. The test_render_error_template() unit test ensures this succeeds.
        let html = ERROR_TEMPLATE.render("error.html", &context).unwrap();
        (status, Html(html)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use axum::{http::Uri, response::IntoResponse};

    #[test]
    fn test_render_error_template() {
        let bad_uri = "http:\\\\back.slash\\woah\\there";
        let error = super::RouteError::InvalidOriginURI {
            uri: bad_uri.to_string(),
            reason: Uri::from_str(bad_uri).unwrap_err(),
            place: super::OriginUriLocation::RefererHeader,
        };
        // We just want this to not panic
        let _ = error.into_response();
    }
}
