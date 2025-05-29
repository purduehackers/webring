use lol_html::{
    element,
    send::{HtmlRewriter, Settings},
};
use std::{
    fmt::{Display, Write as _},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::io::AsyncReadExt;

use axum::http::{Uri, uri::Scheme};
use chrono::Utc;
use futures::TryStreamExt;
use log::info;
use reqwest::{Client, StatusCode};
use sarlacc::Intern;
use tokio::sync::RwLock;
use tokio_util::io::StreamReader;

use crate::webring::CheckLevel;

static ONLINE_CHECK_TTL_MS: i64 = 1000;

// (Last pinged, Was ping successful) — Used to check if the server is online
static PING_INFO: RwLock<(i64, bool)> = RwLock::const_new((i64::MIN, false));

/// If a request succeeds, then call this function to mark the server as definitely online.
async fn mark_server_as_online() {
    let at = Utc::now().timestamp_millis();
    let mut ping_info = PING_INFO.write().await;
    let now = Utc::now().timestamp_millis();

    if at + ONLINE_CHECK_TTL_MS < now || ping_info.0 > at {
        return;
    }

    *ping_info = (at, true);
}

/// Check if the server is online by either getting a cached value (cached for `ONLINE_CHECK_TTL_MS`), or by pinging `8.8.8.8`.
#[allow(dead_code)] // Used in code that may or may not be cfg'd out
async fn is_online() -> bool {
    {
        // Has it been checked within the TTL?
        let ping_info = PING_INFO.read().await;
        let now = Utc::now().timestamp_millis();
        if now < ping_info.0 + ONLINE_CHECK_TTL_MS {
            return ping_info.1;
        }
    }

    // Make sure that we hold the lock so that other threads wait while we're pinging instead of pinging more
    let mut ping_info = PING_INFO.write().await;

    // What if another thread did the ping while we were waiting for the write lock? If so, return it.
    let now = Utc::now().timestamp_millis();
    if now < ping_info.0 + ONLINE_CHECK_TTL_MS {
        return ping_info.1;
    }

    // Ping something.
    // Pings don't work in GitHub Actions runners, so if we're running in GH Actions, just pretend
    // our ping succeeded, since we know we're online. We do this check only in non-release builds
    // so we don't incur the runtime cost of checking the environment variable repeatedly. We don't
    // just check at compile time because we may want to build binaries on GH Actions in the
    // future.
    let ping_successful = if cfg!(debug_assertions)
        && std::env::var("GITHUB_ACTIONS").is_ok_and(|val| &val == "true")
    {
        true
    } else {
        let result = surge_ping::ping("8.8.8.8".parse().unwrap(), &[0; 8]).await;
        result.is_ok()
    };

    // Write the info
    let now = Utc::now().timestamp_millis();
    *ping_info = (now, ping_successful);

    ping_successful
}

/// Checks whether a given URL passes the given check level.
///
/// Returns `Ok(Some(...))` with the failure details if the site fails the check, or `Ok(None)` if it passes.
///
/// Returns `Err` if the check cannot be performed (e.g., due to the server being offline).
pub async fn check(
    website: &Uri,
    check_level: CheckLevel,
    base_address: Intern<Uri>,
) -> Result<Option<CheckFailure>, ()> {
    match check_impl(website, check_level, base_address).await {
        None => Ok(None),
        Some(failure) => {
            // If the issue is not a connection issue, or if it is a connection issue and the
            // server is online, return it. Otherwise, it's a connection issue on our end, so log
            // and count the check as successful.
            #[cfg(not(test))]
            if let CheckFailure::Connection(connection_error) = &failure {
                if !is_online().await {
                    log::error!(
                        "Server-side connectivity issue detected: Could not reach {website}: {connection_error}"
                    );
                    return Err(());
                }
            }

            info!("{website} failed a check: {failure}");
            Ok(Some(failure))
        }
    }
}

async fn check_impl(
    website: &Uri,
    check_level: CheckLevel,
    base_address: Intern<Uri>,
) -> Option<CheckFailure> {
    static CLIENT: LazyLock<Client> = LazyLock::new(|| {
        Client::builder()
            .user_agent(format!(
                "{}/{} (Purdue Hackers webring, +{})",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_REPOSITORY")
            ))
            .build()
            .expect("Creating the HTTP client should not fail")
    });

    if check_level == CheckLevel::None {
        return None;
    }

    let response = match if check_level == CheckLevel::ForLinks {
        CLIENT.get(website.to_string()).send().await
    } else {
        CLIENT.head(website.to_string()).send().await
    } {
        Ok(response) => response,
        Err(err) => return Some(CheckFailure::Connection(err)),
    };
    println!("{response:?}");
    mark_server_as_online().await;
    let successful_response = match response.error_for_status() {
        Ok(r) => r,
        Err(err) => return Some(CheckFailure::ResponseStatus(err.status().unwrap())),
    };

    if check_level == CheckLevel::ForLinks {
        let stream = successful_response.bytes_stream();

        (scan_for_links(
            StreamReader::new(stream.map_err(std::io::Error::other)),
            base_address,
        )
        .await)
            .err()
    } else {
        None
    }
}

/// Represents a failed result of a validation check
#[derive(Debug)]
pub enum CheckFailure {
    /// Failed to connect to the server
    Connection(reqwest::Error),
    /// Site returned a non-2xx response
    ResponseStatus(StatusCode),
    /// Site returned a successful response but is missing the expected links
    MissingLinks(MissingLinks),
    /// IO error
    IOError(std::io::Error),
    /// HTML parsing error
    ParsingError(lol_html::errors::RewritingError),
}

impl CheckFailure {
    /// Construct a message suitable for the site owner about the given check failure. For
    /// a shorter message format suitable for debugging/logging, use the [`Display`] trait.
    #[must_use]
    pub fn to_message(&self) -> String {
        match self {
            CheckFailure::Connection(err) => format!("Connection to your site failed: {err}"),
            CheckFailure::ResponseStatus(status_code) => {
                let mut msg = format!(
                    "Your site returned an error response: {}",
                    status_code.as_u16()
                );
                if let Some(description) = status_code.canonical_reason() {
                    write!(&mut msg, " ({description})").unwrap();
                }
                msg
            }
            CheckFailure::MissingLinks(missing_links) => missing_links.to_string(),
            CheckFailure::IOError(err) => {
                format!("There was an IO error while reading the body of your site: {err}")
            }
            CheckFailure::ParsingError(err) => {
                format!("There was an error parsing your HTML document: {err}")
            }
        }
    }
}

/// Displays this check failure in a short format suitable for debugging/logging but not suitable
/// for sending to a site owner. For that, use [`CheckFailure::to_message()`].
impl Display for CheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckFailure::Connection(error) => write!(f, "Connection error: {error}"),
            CheckFailure::ResponseStatus(status_code) => {
                write!(f, "Site returned {}", status_to_string(*status_code))
            }
            CheckFailure::MissingLinks(missing_links) => {
                let mut missing_link_names = Vec::with_capacity(3);
                if missing_links.home {
                    missing_link_names.push("ring homepage");
                }
                if missing_links.prev {
                    missing_link_names.push("previous site");
                }
                if missing_links.next {
                    missing_link_names.push("next site");
                }
                write!(f, "Missing links: {}", missing_link_names.join(", "))
            }
            CheckFailure::IOError(e) => e.fmt(f),
            CheckFailure::ParsingError(e) => e.fmt(f),
        }
    }
}

/// Format a status code as `Code (Reason String)`, e.g. `404 (Not Found)`.
fn status_to_string(status: StatusCode) -> String {
    let mut msg = status.as_u16().to_string();
    if let Some(description) = status.canonical_reason() {
        write!(&mut msg, " ({description})").unwrap();
    }
    msg
}

#[derive(Debug, PartialEq, Eq)]
pub struct MissingLinks {
    base_address: Intern<Uri>,
    pub home: bool,
    pub next: bool,
    pub prev: bool,
}

impl MissingLinks {
    /// Should be called for every link on the page. If the inputted link matches any of the expected links, mark it as found.
    fn found_link(&mut self, link: &Uri) {
        let authority = self.base_address.authority().unwrap();

        if ![None, Some(&Scheme::HTTPS), Some(&Scheme::HTTP)].contains(&link.scheme()) {
            return;
        }

        if link.authority() != Some(authority) {
            return;
        }

        let path = link.path().trim_matches('/');

        match path {
            "" => self.home = false,
            "next" => self.next = false,
            "prev" | "previous" => self.prev = false,
            _ => {}
        }
    }

    fn all_found(&self) -> bool {
        !self.home && !self.next && !self.prev
    }
}

impl Display for MissingLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let address_string = self.base_address.to_string();
        let address = address_string.strip_suffix('/').unwrap_or(&address_string);
        writeln!(f, "Your site is missing the following links:")?;
        if self.home {
            writeln!(f, "- <{address}>")?;
        }
        if self.next {
            writeln!(f, "- <{address}/next>")?;
        }
        if self.prev {
            writeln!(f, "- <{address}/prev>")?;
        }

        writeln!(
            f,
            "\nWhat to do:
- If your webpage is rendered client-side, ask the administrators to set the validator to only check for your site being online.
- If you don't use anchor tags for the links, add the attribute `data-phwebring=\"prev\"|\"home\"|\"next\"` to the link elements.
- If you think this alert is in error, send a message in #webring."
        )
    }
}

async fn scan_for_links(
    mut webpage: impl tokio::io::AsyncBufRead + Unpin,
    base_address: Intern<Uri>,
) -> Result<(), CheckFailure> {
    // This synchronization primitives should never actually be contented but it convinces Rust that the thing in question is `Send`. That way it can be kept across the `await`.
    let missing_links = Mutex::new(MissingLinks {
        base_address,
        home: true,
        next: true,
        prev: true,
    });
    let done = AtomicBool::new(false);

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!("a[href]", |el| {
                    // Unwrap is OK since we selected for href
                    let Ok(href) = el.get_attribute("href").unwrap().parse::<Uri>() else {
                        return Ok(());
                    };

                    let mut missing_links = missing_links.lock().unwrap();
                    missing_links.found_link(&href);

                    if missing_links.all_found() {
                        done.store(true, Ordering::Relaxed);
                    }

                    Ok(())
                }),
                element!("*[data-phwebring]", |el| {
                    // Unwrap is OK since we selected for data-phwebring
                    let value = el.get_attribute("data-phwebring").unwrap();

                    let mut missing_links = missing_links.lock().unwrap();

                    match &*value {
                        "prev" | "previous" => missing_links.prev = false,
                        "home" => missing_links.home = false,
                        "next" => missing_links.next = false,
                        _ => {}
                    }

                    if missing_links.all_found() {
                        done.store(true, Ordering::Relaxed);
                    }

                    Ok(())
                }),
            ],
            ..Settings::new_send()
        },
        |_: &[u8]| {},
    );

    let mut buf = Box::new([0_u8; 16384]);
    loop {
        if done.load(Ordering::Relaxed) {
            return Ok(());
        }

        let bytes = webpage
            .read_buf(&mut &mut buf[..])
            .await
            .map_err(CheckFailure::IOError)?;

        if bytes == 0 {
            break;
        }

        rewriter
            .write(&buf[0..bytes])
            .map_err(CheckFailure::ParsingError)?;
    }

    drop(rewriter);

    Err(CheckFailure::MissingLinks(
        missing_links.into_inner().unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::Uri, response::Html, routing::get};
    use reqwest::StatusCode;
    use sarlacc::Intern;

    use super::{CheckFailure, CheckLevel, MissingLinks, check, scan_for_links};

    async fn assert_links_gives(
        base_address: &'static str,
        file: &str,
        res: impl Into<Option<(bool, bool, bool)>>,
    ) {
        assert_eq!(
            match scan_for_links(
                file.replace("ADDRESS", base_address).as_bytes(),
                Intern::new(Uri::from_static(base_address))
            )
            .await
            {
                Ok(()) => None,
                Err(CheckFailure::MissingLinks(missing)) => Some(missing),
                e => panic!("{e:?}"),
            },
            res.into().map(|(home, prev, next)| MissingLinks {
                home,
                next,
                prev,
                base_address: Intern::new(Uri::from_static(base_address))
            })
        );
    }

    #[tokio::test]
    async fn all_links() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<body>
                <a href=\"ADDRESS/\"></a>
                <a href=\"ADDRESS/prev\"></a>
                <a href=\"ADDRESS/next\"></a>
            </body>",
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn just_home() {
        assert_links_gives(
            "http://purduehackers.com/",
            "<div>
                <a href=\"ADDRESS\"></a>
            </div>",
            (false, true, true),
        )
        .await;
    }

    #[tokio::test]
    async fn just_prev() {
        assert_links_gives(
            "https://purduehackers.com/",
            "<carousel>
                <a href=\"ADDRESSprev?query=huh\"></a>
            </carousel>",
            (true, false, true),
        )
        .await;
    }

    #[tokio::test]
    async fn just_previous() {
        assert_links_gives(
            "https://x/",
            "<carousel>
                <img/>
            </carousel>
            <thingy>
                <a href=\"ADDRESSprevious?query=huh\"></a>
            </thingy>",
            (true, false, true),
        )
        .await;
    }

    #[tokio::test]
    async fn just_next() {
        assert_links_gives(
            "https://uz/",
            "<body>
                <a href=\"ADDRESSnext/\"></a>
            </body>",
            (true, true, false),
        )
        .await;
    }

    #[tokio::test]
    async fn random_links() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "
            <!-- ADDRESSS -->
            <!-- ADDRESSS/prev -->
            <!-- ADDRESSS/next -->
            <a href=\"ADDRESS/bruh/\"></a>
            <a href=\"https://goggle.com\"></a>
            <a href=\"https://google.com\"></a>
            <a href=\"https://gooolo.com\"></a>
            <a href=\"wherever.wherever/home\"></a>
            <a href=\"wherever.wherever/prev\"></a>
            <a href=\"wherever.wherever/next\"></a>
            <b href=\"ADDRESS\"></b>
            <b href=\"ADDRESS/prev\"></b>
            <b href=\"ADDRESS/next\"></b>",
            (true, true, true),
        )
        .await;
    }

    #[tokio::test]
    async fn alternate_elements_all() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<table>
                <b data-phwebring=\"home\"></b>
                <b data-phwebring=\"next\"></b>
                <b data-phwebring=\"prev\"></b>
            </table>",
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn alternate_elements_just_home() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<div>
                <b data-phwebring=\"home\"></b>
            </div>",
            (false, true, true),
        )
        .await;
    }

    #[tokio::test]
    async fn alternate_elements_just_prev() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<body>
                <b data-phwebring=\"prev\"></b>
            </body>",
            (true, false, true),
        )
        .await;
    }

    #[tokio::test]
    async fn alternate_elements_just_previous() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<body>
                <b data-phwebring=\"previous\"></b>
            </body>",
            (true, false, true),
        )
        .await;
    }

    #[tokio::test]
    async fn alternate_elements_just_next() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<body>
                <b data-phwebring=\"next\"></b>
            </body>",
            (true, true, false),
        )
        .await;
    }

    #[tokio::test]
    async fn check_failure_types() {
        // Start a web server so we can do each kinds of checks
        let server_addr = ("127.0.0.1", 32750);
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();
            let router: Router<()> = Router::new()
                .route("/up", get(async || "Hi there!"))
                .route(
                    "/links",
                    get(async || {
                        Html(
                            r#"
                        <a href="https://ring.purduehackers.com/">Purdue Hackers webring</a>
                        <a href="https://ring.purduehackers.com/prev">Previous site</a>
                        <a href="https://ring.purduehackers.com/next">Next site</a>
                        "#,
                        )
                    }),
                )
                .route(
                    "/error",
                    get(async || {
                        let status = StatusCode::from_u16(rand::random_range(400..600)).unwrap();
                        (status, "Uh oh :(")
                    }),
                );
            axum::serve(listener, router).await.unwrap();
        });

        // Create a site for each endpoint, plus one which will fail to connect.
        // The second value in the tuple is the list of checks for which this member should succeed.
        // The third is the check failure we expect.
        #[expect(clippy::type_complexity)]
        let sites: Vec<(Uri, Vec<CheckLevel>, fn(CheckFailure) -> bool)> = vec![
            (
                Uri::from_static("http://127.0.0.10:60000/connection"),
                vec![CheckLevel::None],
                |failure| matches!(failure, CheckFailure::Connection(_)),
            ),
            (
                Uri::from_static("http://127.0.0.1:32750/error"),
                vec![CheckLevel::None],
                |failure| matches!(failure, CheckFailure::ResponseStatus(_)),
            ),
            (
                Uri::from_static("http://127.0.0.1:32750/up"),
                vec![CheckLevel::None, CheckLevel::JustOnline],
                |failure| matches!(failure, CheckFailure::MissingLinks(_)),
            ),
            (
                Uri::from_static("http://127.0.0.1:32750/links"),
                vec![
                    CheckLevel::None,
                    CheckLevel::JustOnline,
                    CheckLevel::ForLinks,
                ],
                |_| false,
            ),
        ];

        let base = Intern::new(Uri::from_static("https://ring.purduehackers.com"));
        for (site, expect_passing, does_failure_match) in sites {
            let levels = [
                CheckLevel::None,
                CheckLevel::JustOnline,
                CheckLevel::ForLinks,
            ];
            for level in levels {
                // FIXME: Collect CheckFailure and check type
                let maybe_failure = super::check(&site, level, base).await.unwrap();
                eprintln!("Checking {} at level {:?}", &site, level);
                let was_successful = maybe_failure.is_none();
                assert_eq!(expect_passing.contains(&level), was_successful);
                if !was_successful {
                    assert!(does_failure_match(maybe_failure.unwrap()));
                }
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn kians_site() {
        let base = Intern::new(Uri::from_static("https://ring.purduehackers.com"));

        for level in [
            CheckLevel::None,
            CheckLevel::JustOnline,
            CheckLevel::ForLinks,
        ] {
            assert!(
                check(&Uri::from_static("https://kasad.com"), level, base)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
}
