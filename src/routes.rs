//! Routes and endpoints

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Write},
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Query, Request, State},
    handler::HandlerWithoutStateExt,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header, uri::InvalidUri},
    response::{Html, IntoResponse, NoContent, Response},
    routing::get,
};
use log::warn;
use tera::Tera;
use tower_http::{
    catch_panic::{CatchPanicLayer, ResponseForPanic},
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::webring::{TraverseWebringError, Webring};

/// Static HTML template for rendering error responses.
static ERROR_TEMPLATE: LazyLock<Tera> = LazyLock::new(create_error_template);

/// Creates a [`Router`] with the routes for our application.
pub fn create_router(static_dir: &Path) -> Router<Arc<Webring>> {
    let mut router = Router::new()
        .nest_service(
            "/static",
            ServeDir::new(static_dir).fallback(HandlerWithoutStateExt::into_service(
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
        .route("/visit", get(serve_visit))
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

/// Creates a Tera template for rendering error pages.
///
/// The template is baked into the binary at compile time using `include_str!`, so it is guaranteed
/// to be correct and renderable.
fn create_error_template() -> Tera {
    let html_src = include_str!("templates/error.html");
    let css_src = include_str!("templates/error.css");
    let mut tera = Tera::default();
    tera.add_raw_templates(vec![("error.html", html_src), ("error.css", css_src)])
        .unwrap();
    tera
}

/// Create an HTTP 303 redirect to the given URI with HTML content describing the redirect.
fn redirect_with_content(to: &Uri) -> impl IntoResponse + use<> {
    let uri_str = to.to_string();
    let html = format!(
        r#"<!DOCTYPE html><html><body>Redirecting you to <a href="{}">{}</a>...</body></html>"#,
        html_escape::encode_quoted_attribute(&uri_str),
        html_escape::encode_text(&uri_str),
    );
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, uri_str)],
        Html(html),
    )
}

/// Serve the homepage at `/`
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

/// Serve the `/visit` endpoint
///
/// For each request, this function:
/// 1. Gets the authority for the webring member from a URL parameter
/// 2. Simultaneously queries the webring for the member's URL and logs the request in the statistics
/// 3. Redirects the user to the member's page
async fn serve_visit(
    State(webring): State<Arc<Webring>>,
    Query(mut params): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, RouteError> {
    let uri = match params.remove("member") {
        Some(str) => match str.parse::<Uri>() {
            Ok(uri) => uri,
            Err(e) => {
                return Err(RouteError::InvalidRedirectURI {
                    uri: str,
                    reason: e,
                });
            }
        },
        None => return Err(RouteError::MissingRedirectURI),
    };

    let actual_uri = webring.track_from_homepage_click(&uri, addr.ip())?;

    Ok(redirect_with_content(&actual_uri))
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
) -> Result<impl IntoResponse, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.next_page(&origin, addr.ip())?;
    Ok((
        // Set Vary: Referer so browsers will not cache responses for requests with different origins
        [(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Referer"),
        )],
        redirect_with_content(&page),
    ))
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
) -> Result<impl IntoResponse, RouteError> {
    let origin = get_origin_from_request(headers, params)?;
    let page = webring.prev_page(&origin, addr.ip())?;
    Ok((
        // Set Vary: Referer so browsers will not cache responses for requests with different origins
        [(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Referer"),
        )],
        redirect_with_content(&page),
    ))
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
) -> Result<impl IntoResponse, RouteError> {
    let maybe_origin = get_origin_from_request(headers, params).ok();
    let page = webring.random_page(maybe_origin.as_ref(), addr.ip())?;
    Ok((
        // Don't cache since this response can change for every request
        [(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache, no-store"),
        )],
        redirect_with_content(&page),
    ))
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

/// Represents the part of the request where the origin URI was found.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum OriginUriLocation {
    /// The `Referer` header
    RefererHeader,
    /// The `?host=` query parameter
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

/// Represents errors that can occur while routing requests.
#[derive(Debug, thiserror::Error)]
enum RouteError {
    /// An origing URI was needed to find the next/previous site in the ring, but it was invalid.
    #[error(r#"Origin URI "{uri}" found in {place} is invalid: {reason}."#)]
    InvalidOriginURI {
        /// The origin URI that was found to be invalid
        uri: String,
        /// The reason why the URI was invalid
        #[source]
        reason: InvalidUri,
        /// Where in the request the URI was found
        place: OriginUriLocation,
    },

    /// The URI to which the server should redirect the client was invalid.
    #[error(r#"Redirect URI "{uri}" is invalid: {reason}."#)]
    InvalidRedirectURI {
        /// The redirect URI that was found to be invalid
        uri: String,
        /// The reason why the URI was invalid
        #[source]
        reason: InvalidUri,
    },

    /// The request did not contain an origin URI, but it was required, e.g. to find the next/previous site.
    #[error("Request doesn't indicate which site it originates from.")]
    MissingOriginURI,

    /// The request did not contain a redirect URI, but it was required, e.g. to redirect the user to a member's site.
    #[error("Request doesn't indicate which site it it is redirecting to.")]
    MissingRedirectURI,

    /// A header in the request could not be converted to a string because it contained invalid UTF-8 data.
    #[error("{0} contains data which is not valid UTF-8.")]
    HeaderToStr(OriginUriLocation),

    /// An error occurred while traversing the webring.
    #[error("Error traversing webring: {0}")]
    Traversal(
        #[from]
        #[source]
        TraverseWebringError,
    ),

    /// An error occurred while rendering the homepage.
    #[error("Error rendering homepage: {0}.")]
    RenderHomepage(
        #[from]
        #[source]
        eyre::Report,
    ),

    /// No route was found for the given request path.
    #[error("No handler for path {0}")]
    NoRoute(Uri),

    /// A static file was requested, but it does not exist.
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
            | RouteError::InvalidRedirectURI { .. }
            | RouteError::MissingOriginURI
            | RouteError::MissingRedirectURI
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

/// A response type that can be used to render an error page when a panic occurs.
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
    use std::{net::SocketAddr, str::FromStr as _, sync::Arc};

    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        http::{Request, Uri, header},
        response::IntoResponse,
    };
    use chrono::Utc;
    use eyre::eyre;
    use http_body_util::BodyExt;
    use indoc::indoc;
    use pretty_assertions::assert_ne;
    use reqwest::StatusCode;
    use tempfile::TempDir;
    use tokio::fs;
    use tower::{Service, ServiceExt};
    use tower_http::catch_panic::ResponseForPanic;

    use crate::{
        stats::{TIMEZONE, UNKNOWN_ORIGIN},
        webring::Webring,
    };

    use super::{OriginUriLocation, PanicResponse, RouteError, create_router};

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

    // Return the temporary files so that they don't go away
    async fn app() -> (Router, Arc<Webring>, TempDir) {
        let static_dir = TempDir::new().unwrap();
        fs::write(static_dir.path().join("index.html"), "Hello homepage!")
            .await
            .unwrap();
        let config = toml::from_str(&format!(indoc! { r#"
            [webring]
            base-url = "https://ring.purduehackers.com"
            static-dir = "{}"
            [network]
            listen-addr = "0.0.0.0:3000"
            [members]
            henry = {{ url = "hrovnyak.gitlab.io", discord-id = 123, check-level = "none" }}
            kian = {{ url = "kasad.com", discord-id = 456, check-level = "none" }}
            cynthia = {{ url = "https://clementine.viridian.page", discord-id = 789, check-level = "none" }}
            "???" = {{ url = "ws://refuse-the-r.ing", check-level = "none" }}
        "# }, static_dir.path().display())).unwrap();
        let webring = Arc::new(Webring::new(&config));
        let router: Router = create_router(static_dir.path()).with_state(Arc::clone(&webring));
        (router, webring, static_dir)
    }

    #[tokio::test]
    async fn index() {
        let (router, webring, tmpfiles) = app().await;

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        // Request `/`
        let res = router
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Referer", "kasad.com")
                    .extension(ConnectInfo("1.2.3.5:433".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        assert_eq!("Hello homepage!", text);
        assert_eq!(status, StatusCode::OK);
        webring.assert_stat_entry(
            (today, "kasad.com", "ring.purduehackers.com", "kasad.com"),
            1,
        );

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn index_unknown_referer() {
        let (router, webring, tmpfiles) = app().await;

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Referer", "https://who.now/")
                    .extension(ConnectInfo("1.2.3.2:433".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        assert_eq!("Hello homepage!", text);
        assert_eq!(status, StatusCode::OK);
        webring.assert_stat_entry(
            (
                today,
                UNKNOWN_ORIGIN.as_str(),
                "ring.purduehackers.com",
                UNKNOWN_ORIGIN.as_str(),
            ),
            1,
        );

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn visit() {
        let (router, webring, tmpfiles) = app().await;

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/visit?member=clementine.viridian.page")
                    .extension(ConnectInfo("5.4.3.2:80".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers().get("location").unwrap(),
            "https://clementine.viridian.page/"
        );
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        webring.assert_stat_entry(
            (
                today,
                "ring.purduehackers.com",
                "clementine.viridian.page",
                "ring.purduehackers.com",
            ),
            1,
        );

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn visit_unknown_member() {
        let (router, _, tmpfiles) = app().await;

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/visit?member=iamtheyeastofthoughtsandm.ind")
                    .extension(ConnectInfo("5.4.3.2:80".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn next() {
        let (router, _, tmpfiles) = app().await;

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/next?host=clementine.viridian.page")
                    .extension(ConnectInfo("5.4.3.2:80".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers().get("location").unwrap(),
            "ws://refuse-the-r.ing/"
        );
        assert_eq!(res.status(), StatusCode::SEE_OTHER);

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn prev() {
        let (router, _, tmpfiles) = app().await;

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/prev?host=https://clementine.viridian.page")
                    .header("Referer", "kasad.com")
                    .extension(ConnectInfo("5.4.3.2:80".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers().get("location").unwrap(), "kasad.com");
        assert_eq!(res.status(), StatusCode::SEE_OTHER);

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn previous() {
        let (router, _, tmpfiles) = app().await;

        let res = router
            .oneshot(
                Request::builder()
                    .uri("/previous")
                    .header("Referer", "kasad.com")
                    .extension(ConnectInfo("5.4.3.2:80".parse::<SocketAddr>().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers().get("location").unwrap(), "hrovnyak.gitlab.io");
        assert_eq!(res.status(), StatusCode::SEE_OTHER);

        drop(tmpfiles);
    }

    #[tokio::test]
    async fn redirect_contains_body() {
        let paths = [
            "/visit?member=kasad.com",
            "/next?host=kasad.com",
            "/prev?host=kasad.com",
            "/previous?host=kasad.com",
            "/random",
        ];
        let (router, _webring, _tmpfiles) = app().await;
        let mut s = router.into_service::<Body>();
        for path in paths {
            let res = s
                .call(
                    Request::builder()
                        .uri(path)
                        .extension(ConnectInfo::<SocketAddr>("5.4.3.2:38452".parse().unwrap()))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/html; charset=utf-8"
            );
            assert_ne!(res.headers().get(header::CONTENT_LENGTH).unwrap(), "0");
        }
    }
}
