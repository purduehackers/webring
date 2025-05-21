//! Routes and endpoints

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Write},
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Query, Request, State},
    handler::HandlerWithoutStateExt,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, uri::InvalidUri},
    response::{Html, IntoResponse, NoContent, Redirect, Response},
    routing::get,
};
use log::warn;
use tera::Tera;
use tower_http::{
    catch_panic::{CatchPanicLayer, ResponseForPanic},
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::{
    CliOptions,
    webring::{TraverseWebringError, Webring},
};

static ERROR_TEMPLATE: LazyLock<Tera> = LazyLock::new(create_error_template);

/// Creates a [`Router`] with the routes for our application.
pub fn create_router(cli: &CliOptions) -> Router<Arc<Webring>> {
    let mut router = Router::new()
        .nest_service(
            "/static",
            ServeDir::new(&cli.static_dir).fallback(HandlerWithoutStateExt::into_service(
                async |request: Request| -> RouteError {
                    RouteError::FileNotFound(format!("/static{}", request.uri()))
                },
            )),
        )
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
        .fallback_service(HandlerWithoutStateExt::into_service(
            async |request: Request| -> RouteError {
                RouteError::NoRoute(request.uri().to_owned())
            },
        ));

    // Add debugging routes
    if cfg!(debug_assertions) {
        router = router.route(
            "/debug/panic",
            get(
                #[allow(clippy::unused_unit)]
                async || -> () {
                    panic!();
                },
            ),
        );
    }

    router.layer(CatchPanicLayer::custom(PanicResponse))
}

fn create_error_template() -> Tera {
    let html_src = include_str!("templates/error.html");
    let css_src = include_str!("templates/error.css");
    let mut tera = Tera::default();
    tera.add_raw_templates(vec![("error.html", html_src), ("error.css", css_src)])
        .unwrap();
    tera
}

async fn serve_index(
    State(webring): State<Arc<Webring>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, RouteError> {
    let maybe_origin = get_origin_from_request(headers, params).ok();
    webring.track_to_homepage_click(maybe_origin.as_ref(), addr.ip());
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
    State(webring): State<Arc<Webring>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.next_page(&origin, addr.ip())?;
    Ok((
        // Set Vary: Referer so browsers will not cache responses for requests with different origins
        [(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Referer"),
        )],
        Redirect::to(page.to_string().as_str()),
    )
        .into_response())
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
    State(webring): State<Arc<Webring>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.prev_page(&origin, addr.ip())?;
    Ok((
        // Set Vary: Referer so browsers will not cache responses for requests with different origins
        [(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Referer"),
        )],
        Redirect::to(page.to_string().as_str()),
    )
        .into_response())
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
    State(webring): State<Arc<Webring>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, RouteError> {
    let maybe_origin = get_origin_from_request(headers, params).ok();
    let page = webring.random_page(maybe_origin.as_ref(), addr.ip())?;
    Ok((
        // Don't cache since this response can change for every request
        [(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache, no-store"),
        )],
        Redirect::to(page.to_string().as_str()),
    )
        .into_response())
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
    #[error(r#"Origin URI "{uri}" found in {place} is invalid: {reason}."#)]
    InvalidOriginURI {
        uri: String,
        #[source]
        reason: InvalidUri,
        place: OriginUriLocation,
    },

    #[error("Request doesn't indicate which site it originates from.")]
    MissingOriginURI,

    #[error("{0} contains data which is not valid UTF-8.")]
    HeaderToStr(OriginUriLocation),

    #[error("Error traversing webring: {0}")]
    Traversal(
        #[from]
        #[source]
        TraverseWebringError,
    ),

    #[error("Error rendering homepage: {0}.")]
    RenderHomepage(
        #[from]
        #[source]
        eyre::Report,
    ),

    #[error("No handler for path {0}")]
    NoRoute(Uri),

    #[error("File {0} does not exist")]
    FileNotFound(String),
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
            RouteError::NoRoute(_) | RouteError::FileNotFound(_) => StatusCode::NOT_FOUND,
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
                StatusCode::NOT_FOUND => "The page you requested doesn't exist.",
                other => {
                    warn!(
                        "Error code without registered message was returned: {} ({})",
                        other.as_u16(),
                        other.canonical_reason().unwrap_or("")
                    );
                    "We're not quite sure."
                }
            },
        );
        context.insert("cargo_pkg_repository", env!("CARGO_PKG_REPOSITORY"));

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

#[derive(Copy, Clone, Debug)]
struct PanicResponse;

impl ResponseForPanic for PanicResponse {
    type ResponseBody = Body;

    fn response_for_panic(
        &mut self,
        err: Box<dyn std::any::Any + Send + 'static>,
    ) -> axum::http::Response<Self::ResponseBody> {
        let mut panic_message = String::new();
        if let Some(name) = std::thread::current().name() {
            write!(
                &mut panic_message,
                "Thread {name} panicked while generating a response: "
            )
            .unwrap();
        } else {
            write!(
                &mut panic_message,
                "Thread panicked while generating a response: "
            )
            .unwrap();
        }
        if let Some(str) = err.downcast_ref::<&str>() {
            panic_message.push_str(str);
        } else if let Some(string) = err.downcast_ref::<String>() {
            panic_message.push_str(string);
        }
        panic_message.push('\n');

        let bt = std::backtrace::Backtrace::force_capture();
        write!(&mut panic_message, "{bt}").unwrap();

        let status = StatusCode::INTERNAL_SERVER_ERROR;
        let mut ctx = tera::Context::new();

        ctx.insert("status_code", &status.as_u16());
        ctx.insert("status_text", status.canonical_reason().unwrap());
        ctx.insert("description", "Something unexpected went wrong.");
        ctx.insert("cargo_pkg_repository", env!("CARGO_PKG_REPOSITORY"));
        ctx.insert("error", &panic_message);

        let html = ERROR_TEMPLATE.render("error.html", &ctx).unwrap();
        (status, Html(html)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use axum::{http::Uri, response::IntoResponse};
    use eyre::eyre;
    use tower_http::catch_panic::ResponseForPanic;

    use super::{OriginUriLocation, PanicResponse, RouteError};

    /// Test rendering src/templates/error.html to make sure it won't panic at runtime.
    #[test]
    fn test_render_error_template() {
        // Try a 400 error
        let bad_uri = "http:\\\\back.slash\\woah\\there";
        let error = RouteError::InvalidOriginURI {
            uri: bad_uri.to_string(),
            reason: Uri::from_str(bad_uri).unwrap_err(),
            place: OriginUriLocation::RefererHeader,
        };
        let _ = error.into_response();

        // Try a 500 error
        let error = RouteError::RenderHomepage(eyre!("Fake rendering error"));
        let _ = error.into_response();
    }

    /// Test rendering src/templates/error.html from a panic to make sure it won't panic at
    /// runtime.
    #[test]
    fn test_render_panic_response() {
        let mut pr = PanicResponse;
        let _ = pr.response_for_panic(Box::new("intentional panic"));
    }
}
