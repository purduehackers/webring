//! Handles checking the webring sites for compliance with the webring requirements.

use indoc::indoc;
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
    time::Duration,
};

use axum::{
    body::Bytes,
    http::{Uri, uri::Scheme},
};
use chrono::Utc;
use futures::{Stream, TryStreamExt};
use reqwest::{Client, Response, StatusCode};
use sarlacc::Intern;
use tokio::sync::RwLock;
use tracing::{error, info, instrument};

use crate::{discord::Snowflake, webring::CheckLevel};

/// Discord ID of the #webring channel
// TODO: We might want to move this to the config file
#[allow(clippy::unreadable_literal)]
const WEBRING_CHANNEL: Snowflake = Snowflake::new(1319140464812753009);

/// The time in milliseconds for which the server is considered online after a successful ping.
const ONLINE_CHECK_TTL_MS: i64 = 1000;

/// The timeout to retry requesting a site after failure
const RETRY_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times to attempt to retry a connection after failure
const RETRY_COUNT: usize = 5;

/// How long requests will wait before failing due to timing out
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The HTTP client used to make requests to the webring sites for validation.
static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .user_agent(format!(
            "{}/{} (Purdue Hackers webring, +{})",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_REPOSITORY")
        ))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("Creating the HTTP client should not fail")
});

/// (Last pinged, Was ping successful) — Used to check if the server is online
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

/// Check if the server is online by either getting a cached value (cached for `ONLINE_CHECK_TTL_MS`), or by requesting our repository.
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

    // Head-request our repository to make sure we're online.
    let result = CLIENT
        .head(env!("CARGO_PKG_REPOSITORY"))
        .send()
        .await
        .and_then(Response::error_for_status);
    let ping_successful = result.is_ok();

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
            if let CheckFailure::Connection(connection_error) = &failure {
                if !is_online().await {
                    error!(
                        site = %website,
                        err = %connection_error,
                        "Server-side connectivity issue detected: could not reach site"
                    );
                    return Err(());
                }
            }

            info!(site = %website, ?failure, "site failed a check");
            Ok(Some(failure))
        }
    }
}

/// Perform the actual check for the given website and check level.
///
/// If the site fails any check, returns `Some(CheckFailure)`.
/// If the site passes all checks, returns `None`.
#[instrument(skip(base_address))]
async fn check_impl(
    website: &Uri,
    check_level: CheckLevel,
    base_address: Intern<Uri>,
) -> Option<CheckFailure> {
    if check_level == CheckLevel::None {
        return None;
    }

    let mut response;

    let mut retry_limit = RETRY_COUNT;

    loop {
        response = if check_level == CheckLevel::ForLinks {
            CLIENT.get(website.to_string()).send().await
        } else {
            CLIENT.head(website.to_string()).send().await
        };

        if retry_limit == 0 {
            break;
        }

        match &response {
            Ok(_) => break,
            Err(err) => {
                info!(
                    site = %website, %err, delay = ?RETRY_TIMEOUT, "Error requesting site; retrying after delay"
                );
            }
        }

        retry_limit -= 1;

        if !cfg!(test) {
            tokio::time::sleep(RETRY_TIMEOUT).await;
        }
    }

    let response = match response {
        Ok(v) => v,
        Err(err) => return Some(CheckFailure::Connection(err)),
    };

    mark_server_as_online().await;
    let successful_response = match response.error_for_status() {
        Ok(r) => r,
        Err(err) => return Some(CheckFailure::ResponseStatus(err.status().unwrap())),
    };

    if check_level == CheckLevel::ForLinks {
        let stream = successful_response.bytes_stream();

        scan_for_links(stream.map_err(std::io::Error::other), base_address)
            .await
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
    /// Site returned a successful response but has issues with its webring links
    LinkIssues(LinkStatuses),
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
            CheckFailure::LinkIssues(issues) => issues.to_message(),
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
            CheckFailure::LinkIssues(links) => links.fmt(f),
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

/// Represents the status of all expected links on a member site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStatuses {
    /// Base address used for [`to_message()`][LinkStatuses::to_message].
    base_address: Intern<Uri>,
    /// Status of the link to the webring homepage
    pub home: LinkStatus,
    /// Status of the link to the next member's site
    pub next: LinkStatus,
    /// Status of the link to the previous member's site
    pub prev: LinkStatus,
    /// Path of the previous link, if it was found.
    /// This is used to render the message since there are two possible paths, `/prev` and `/previous`.
    prev_path: Option<&'static str>,
}

/// Represents the status of one of the expected links on a member site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    /// The link is ok
    Ok,
    /// The link is missing
    Missing,
    /// The link is found but has a `target="..."` attribute
    HasTarget,
}

impl LinkStatuses {
    /// Should be called for every link on the page.
    /// If the given link matches any of the expected links, mark it as found.
    fn found_link(&mut self, link: &Uri, has_target: bool) {
        let authority = self.base_address.authority().unwrap();

        if ![None, Some(&Scheme::HTTPS), Some(&Scheme::HTTP)].contains(&link.scheme()) {
            return;
        }

        if link.authority() != Some(authority) {
            return;
        }

        let path = link.path().trim_matches('/');

        let status = if has_target {
            LinkStatus::HasTarget
        } else {
            LinkStatus::Ok
        };
        match path {
            // Special case: don't complain about `target=` attributes on the homepage link
            "" => self.home = LinkStatus::Ok,
            "next" => self.next = status,
            "prev" => {
                self.prev = status;
                self.prev_path = Some("/prev");
            }
            "previous" => {
                self.prev = status;
                self.prev_path = Some("/previous");
            }
            _ => (),
        }
    }

    /// Returns `true` if all expected links are [ok][LinkStatus::Ok].
    fn all_ok(&self) -> bool {
        [self.home, self.next, self.prev]
            .into_iter()
            .all(|status| status == LinkStatus::Ok)
    }

    /// Returns `true` if any of the expected links are missing.
    fn any_missing(&self) -> bool {
        [self.home, self.next, self.prev]
            .into_iter()
            .any(|status| status == LinkStatus::Missing)
    }

    /// Returns `true` if any of the expected links has a `target="..."` attribute.
    fn any_have_target(&self) -> bool {
        [self.home, self.next, self.prev]
            .into_iter()
            .any(|status| status == LinkStatus::HasTarget)
    }

    /// Construct a message suitable for the site owner about the given link issues. For
    /// a shorter message format suitable for debugging/logging, use the [`Display`] trait.
    fn to_message(&self) -> String {
        let mut msg = String::new();
        let address_string = self.base_address.to_string();
        let address = address_string.strip_suffix('/').unwrap_or(&address_string);
        msg.push_str("Your site's webring links have the following issues:\n");
        let statuses_and_paths = [
            (self.home, ""),
            (self.next, "/next"),
            (self.prev, self.prev_path.unwrap_or("/prev")),
        ];
        for (status, path) in statuses_and_paths {
            match status {
                LinkStatus::Ok => (),
                LinkStatus::Missing => {
                    writeln!(msg, "- Link to <{address}{path}> is missing").unwrap();
                }
                LinkStatus::HasTarget => {
                    writeln!(
                        msg,
                        "- Link to <{address}{path}> has a `target=\"...\"` attribute"
                    )
                    .unwrap();
                }
            }
        }

        msg.push_str("\nWhat to do:\n");
        if self.any_missing() {
            msg.push_str(indoc! {
                r#"
                - If your webpage is rendered client-side, ask the administrators to set the validator to only check for your site being online.
                - If you don't use anchor tags for the links, add the attribute `data-phwebring="prev"|"home"|"next"` to the link elements.
                "#
            });
        }
        if self.any_have_target() {
            msg.push_str("- Don't include a `target` attribute on the links.\n");
        }
        writeln!(
            &mut msg,
            "- If you think this alert is in error, send a message in <#{WEBRING_CHANNEL}>."
        )
        .unwrap();
        msg
    }
}

impl Display for LinkStatuses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let statuses_and_names = [
            (self.home, "ring homepage"),
            (self.next, "next site"),
            (self.prev, "previous site"),
        ];
        let mut missing = Vec::with_capacity(3);
        let mut has_target = Vec::with_capacity(3);
        for (status, name) in statuses_and_names {
            match status {
                LinkStatus::Ok => (),
                LinkStatus::Missing => missing.push(name.to_owned()),
                LinkStatus::HasTarget => has_target.push(name.to_owned()),
            }
        }
        match (missing.is_empty(), has_target.is_empty()) {
            (true, true) => write!(f, "Links are valid"),
            (false, true) => write!(f, "Missing links: {}", missing.join(", ")),
            (true, false) => write!(f, "Links with target attribute: {}", has_target.join(", ")),
            (false, false) => write!(
                f,
                "Missing links: {}; links with target attribute: {}",
                missing.join(", "),
                has_target.join(", ")
            ),
        }
    }
}

/// Parse the HTML of a webpage and scan it for the expected links.
async fn scan_for_links(
    mut webpage: impl Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
    base_address: Intern<Uri>,
) -> Result<(), CheckFailure> {
    // This synchronization primitives should never actually be contended but it convinces Rust
    // that the thing in question is `Send`. That way it can be kept across the `await`.
    let links = Mutex::new(LinkStatuses {
        base_address,
        home: LinkStatus::Missing,
        next: LinkStatus::Missing,
        prev: LinkStatus::Missing,
        prev_path: None,
    });
    let done = AtomicBool::new(false);

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![
                element!("a[href], area[href]", |el| {
                    // Unwrap is OK since we selected for href
                    let Ok(href) =
                        html_escape::decode_html_entities(&el.get_attribute("href").unwrap())
                            .trim()
                            .parse::<Uri>()
                    else {
                        return Ok(());
                    };
                    let has_target = el.get_attribute("target").is_some();

                    let mut links = links.lock().unwrap();
                    links.found_link(&href, has_target);

                    if links.all_ok() {
                        done.store(true, Ordering::Relaxed);
                    }

                    Ok(())
                }),
                element!("*[data-phwebring]", |el| {
                    // Unwrap is OK since we selected for data-phwebring
                    let attr = el.get_attribute("data-phwebring").unwrap();
                    let decoded = html_escape::decode_html_entities(&attr);
                    let value = decoded.trim();

                    let status = match el.get_attribute("target") {
                        Some(_) => LinkStatus::HasTarget,
                        None => LinkStatus::Ok,
                    };

                    let mut links = links.lock().unwrap();
                    match value {
                        "prev" => {
                            links.prev = status;
                            links.prev_path = Some("/prev");
                        }
                        "previous" => {
                            links.prev = status;
                            links.prev_path = Some("/previous");
                        }
                        "home" => links.home = status,
                        "next" => links.next = status,
                        _ => (),
                    }

                    if links.all_ok() {
                        done.store(true, Ordering::Relaxed);
                    }

                    Ok(())
                }),
            ],
            ..Settings::new_send()
        },
        |_: &[u8]| {},
    );

    loop {
        if done.load(Ordering::Relaxed) {
            return Ok(());
        }

        let Some(bytes) = webpage.try_next().await.map_err(CheckFailure::IOError)? else {
            break;
        };

        rewriter.write(&bytes).map_err(CheckFailure::ParsingError)?;
    }

    drop(rewriter);

    Err(CheckFailure::LinkIssues(links.into_inner().unwrap()))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::Duration,
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        http::Uri,
        response::{Html, Response},
        routing::get,
    };
    use futures::stream;
    use indoc::formatdoc;
    use pretty_assertions::assert_eq;
    use reqwest::StatusCode;
    use sarlacc::Intern;

    use crate::checking::REQUEST_TIMEOUT;

    use super::{
        CheckFailure, CheckLevel, LinkStatus, LinkStatuses, WEBRING_CHANNEL, check, scan_for_links,
    };

    async fn assert_links_gives(
        base_address: &'static str,
        file: &str,
        res: impl Into<Option<(LinkStatus, LinkStatus, LinkStatus)>>,
    ) {
        assert_eq!(
            match scan_for_links(
                stream::once(Box::pin(async {
                    Ok(Bytes::from(file.replace("ADDRESS", base_address)))
                })),
                Intern::new(Uri::from_static(base_address))
            )
            .await
            {
                Ok(()) => None,
                Err(CheckFailure::LinkIssues(mut missing)) => {
                    missing.prev_path = None; // We don't care about the path for these tests
                    Some(missing)
                }
                e => panic!("{e:?}"),
            },
            res.into().map(|(home, prev, next)| LinkStatuses {
                home,
                next,
                prev,
                base_address: Intern::new(Uri::from_static(base_address)),
                prev_path: None,
            })
        );
    }

    #[tokio::test]
    async fn all_links() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            "<body>
                <a href=\"https:&#x2f;&#x2f;ring.purduehackers.com/\"></a>
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
                <area href=\"ADDRESS\"></area>
            </div>",
            (LinkStatus::Ok, LinkStatus::Missing, LinkStatus::Missing),
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
            (LinkStatus::Missing, LinkStatus::Ok, LinkStatus::Missing),
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
                <a href=\" \n     \t  ADDRESSprevious?query=huh   \t\n  \"></a>
            </thingy>",
            (LinkStatus::Missing, LinkStatus::Ok, LinkStatus::Missing),
        )
        .await;
    }

    #[tokio::test]
    async fn just_next() {
        assert_links_gives(
            "https://uz/",
            "<body>
                <area href=\"ADDRESSnext/\"></area>
            </body>",
            (LinkStatus::Missing, LinkStatus::Missing, LinkStatus::Ok),
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
            (
                LinkStatus::Missing,
                LinkStatus::Missing,
                LinkStatus::Missing,
            ),
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
                <b data-phwebring=\"\n \t    home   \t     \n  \"></b>
            </div>",
            (LinkStatus::Ok, LinkStatus::Missing, LinkStatus::Missing),
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
            (LinkStatus::Missing, LinkStatus::Ok, LinkStatus::Missing),
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
            (LinkStatus::Missing, LinkStatus::Ok, LinkStatus::Missing),
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
            (LinkStatus::Missing, LinkStatus::Missing, LinkStatus::Ok),
        )
        .await;
    }

    #[tokio::test]
    async fn has_target() {
        assert_links_gives(
            "https://ring.purduehackers.com",
            r#"<body>
                <a href="ADDRESS" target="_blank"></a>
                <a href="ADDRESS/next" target></a>
            </body>"#,
            (
                LinkStatus::Ok, // Special case: homepage link is ok
                LinkStatus::Missing,
                LinkStatus::HasTarget,
            ),
        )
        .await;
    }

    #[test]
    fn link_statuses_display() {
        use LinkStatus::*;

        let subtests = [
            // (home, prev, next, expected)
            (Ok, Ok, Ok, "Links are valid"),
            (Missing, Ok, Ok, "Missing links: ring homepage"),
            (Ok, Missing, Ok, "Missing links: previous site"),
            (Ok, Ok, Missing, "Missing links: next site"),
            (
                HasTarget,
                Ok,
                Ok,
                "Links with target attribute: ring homepage",
            ),
            (
                Ok,
                HasTarget,
                Ok,
                "Links with target attribute: previous site",
            ),
            (Ok, Ok, HasTarget, "Links with target attribute: next site"),
            (
                HasTarget,
                Missing,
                HasTarget,
                "Missing links: previous site; links with target attribute: ring homepage, next site",
            ),
        ];

        for (home, prev, next, expected) in subtests {
            let statuses = LinkStatuses {
                base_address: Intern::new(Uri::from_static("https://ring.purduehackers.com")),
                home,
                next,
                prev,
                prev_path: None,
            };
            assert_eq!(expected, statuses.to_string());
        }
    }

    #[test]
    fn test_link_status_message() {
        let links = LinkStatuses {
            base_address: Intern::new(Uri::from_static("https://ring.purduehackers.com")),
            home: LinkStatus::Missing,
            next: LinkStatus::Ok,
            prev: LinkStatus::HasTarget,
            prev_path: Some("/previous"),
        };
        let expected = formatdoc! {
            r#"
            Your site's webring links have the following issues:
            - Link to <https://ring.purduehackers.com> is missing
            - Link to <https://ring.purduehackers.com/previous> has a `target="..."` attribute

            What to do:
            - If your webpage is rendered client-side, ask the administrators to set the validator to only check for your site being online.
            - If you don't use anchor tags for the links, add the attribute `data-phwebring="prev"|"home"|"next"` to the link elements.
            - Don't include a `target` attribute on the links.
            - If you think this alert is in error, send a message in <#{WEBRING_CHANNEL}>.
            "#
        };
        assert_eq!(expected, links.to_message());

        let links = LinkStatuses {
            base_address: Intern::new(Uri::from_static("https://ring.purduehackers.com")),
            home: LinkStatus::Ok,
            next: LinkStatus::Ok,
            prev: LinkStatus::HasTarget,
            prev_path: Some("/previous"),
        };
        let expected = formatdoc! {
            r#"
            Your site's webring links have the following issues:
            - Link to <https://ring.purduehackers.com/previous> has a `target="..."` attribute

            What to do:
            - Don't include a `target` attribute on the links.
            - If you think this alert is in error, send a message in <#{WEBRING_CHANNEL}>.
            "#
        };
        assert_eq!(expected, links.to_message());

        let links = LinkStatuses {
            base_address: Intern::new(Uri::from_static("https://ring.purduehackers.com")),
            home: LinkStatus::Missing,
            next: LinkStatus::Missing,
            prev: LinkStatus::Ok,
            prev_path: Some("/previous"),
        };
        let expected = formatdoc! {
            r#"
            Your site's webring links have the following issues:
            - Link to <https://ring.purduehackers.com> is missing
            - Link to <https://ring.purduehackers.com/next> is missing

            What to do:
            - If your webpage is rendered client-side, ask the administrators to set the validator to only check for your site being online.
            - If you don't use anchor tags for the links, add the attribute `data-phwebring="prev"|"home"|"next"` to the link elements.
            - If you think this alert is in error, send a message in <#{WEBRING_CHANNEL}>.
            "#
        };
        assert_eq!(expected, links.to_message());
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
                Uri::from_static("http://127.0.0.1:60000/connection"),
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
                |failure| matches!(failure, CheckFailure::LinkIssues(_)),
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
    async fn test_retrying() {
        // Start a web server that fails only the first request
        let server_addr = ("127.0.0.1", 32752);

        let ok_hits = Arc::new(AtomicU64::new(0));
        let err_hits = Arc::new(AtomicU64::new(0));
        let already_requested = Arc::new(AtomicBool::new(false));

        let ok_hits_for_server = Arc::clone(&ok_hits);
        let err_hits_for_server = Arc::clone(&err_hits);
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();
            let router = Router::new().route(
                "/up",
                get(async move || {
                    if already_requested.swap(true, Ordering::Relaxed) {
                        ok_hits_for_server.fetch_add(1, Ordering::Relaxed);
                        Response::builder()
                            .status(200)
                            .body(Body::from("Hi there!"))
                            .unwrap()
                    } else {
                        err_hits_for_server.fetch_add(1, Ordering::Relaxed);
                        // Trigger the request timeout
                        let sleep = tokio::time::sleep(Duration::from_secs(1) + REQUEST_TIMEOUT);

                        // we don't want to wait for realsies
                        tokio::time::pause();
                        tokio::time::advance(REQUEST_TIMEOUT - Duration::from_millis(100)).await;
                        tokio::time::resume();

                        sleep.await;
                        Response::builder()
                            .status(500)
                            .body(Body::from("Retry plz!"))
                            .unwrap()
                    }
                }),
            );
            axum::serve(listener, router).await.unwrap();
        });

        let base = Intern::new(Uri::from_static("https://ring.purduehackers.com"));

        let maybe_failure = super::check(
            &Uri::from_static("http://127.0.0.1:32752/up"),
            CheckLevel::JustOnline,
            base,
        )
        .await
        .unwrap();

        assert!(maybe_failure.is_none(), "{maybe_failure:?}");
        assert_eq!(err_hits.load(Ordering::Relaxed), 1);
        assert_eq!(ok_hits.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    #[ignore = "Kian's site could go down"]
    async fn kians_site() {
        let base = Intern::new(Uri::from_static("https://ring.purduehackers.com"));

        for level in [
            CheckLevel::None,
            CheckLevel::JustOnline,
            CheckLevel::ForLinks,
        ] {
            let failure = check(&Uri::from_static("https://kasad.com"), level, base)
                .await
                .unwrap();
            assert!(failure.is_none(), "{}", failure.unwrap());
        }
    }
}
