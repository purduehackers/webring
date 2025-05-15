use std::{error::Error, fmt::Display, io::ErrorKind, sync::LazyLock};

use axum::{
    body::Bytes,
    http::{
        Uri,
        uri::{Authority, PathAndQuery, Scheme},
    },
};
use chrono::Utc;
use eyre::Report;
use futures::{Stream, StreamExt, TryStreamExt};
use log::{error, info};
use quick_xml::{Reader, events::Event};
use tokio::sync::RwLock;
use tokio_util::io::StreamReader;

use crate::webring::CheckLevel;

static ADDRESS: &str = "https://ring.purduehackers.com";

static ONLINE_CHECK_TTL_MS: i64 = 1000;

static PING_INFO: RwLock<(i64, bool)> = RwLock::const_new((i64::MIN, false));

async fn server_definitely_online() {
    let at = Utc::now().timestamp_millis();
    let mut ping_info = PING_INFO.write().await;
    let now = Utc::now().timestamp_millis();

    if at + ONLINE_CHECK_TTL_MS < now || ping_info.0 > at {
        return;
    }

    *ping_info = (at, true);
}

async fn is_online() -> bool {
    {
        let ping_info = PING_INFO.read().await;
        let now = Utc::now().timestamp_millis();
        if now < ping_info.0 + 1000 {
            return ping_info.1;
        }
    }

    let mut ping_info = PING_INFO.write().await;

    let now = Utc::now().timestamp_millis();
    if now < ping_info.0 + ONLINE_CHECK_TTL_MS {
        return ping_info.1;
    }

    let status = surge_ping::ping("8.8.8.8".parse().unwrap(), &[0; 8]).await;

    let now = Utc::now().timestamp_millis();
    *ping_info = (now, status.is_ok());

    ping_info.1
}

/// Checks whether a given URL passes the given check level and sends of discord messages if not.
///
/// Returns Some with the result, or None if a connectivity issue on the server side is detected.
#[allow(clippy::unused_async)]
pub async fn check(website: &Uri, check_level: CheckLevel) -> Option<bool> {
    match check_impl(website, check_level).await {
        Ok(()) => Some(true),
        Err(e) => {
            // TODO: Discord

            if is_online().await {
                info!("{website} failed a check: {e}");
                Some(false)
            } else {
                error!("Server side connectivity issue detected!");
                None
            }
        }
    }
}

async fn check_impl(website: &Uri, check_level: CheckLevel) -> eyre::Result<()> {
    match check_level {
        CheckLevel::ForLinks => {
            let res = reqwest::get(website.to_string())
                .await?
                .error_for_status()?;
            server_definitely_online().await;

            let stream = res.bytes_stream();

            match contains_link(stream).await {
                None => Ok(()),
                Some(links) => Err(Report::new(links)),
            }
        }
        CheckLevel::JustOnline => {
            reqwest::get(website.to_string())
                .await?
                .error_for_status()?;
            server_definitely_online().await;

            Ok(())
        }
        CheckLevel::None => Ok(()),
    }
}

#[derive(Debug)]
pub struct MissingLinks {
    pub home: bool,
    pub next: bool,
    pub prev: bool,
}

impl MissingLinks {
    fn found_link(&mut self, link: &Uri) {
        static LINKS: LazyLock<(Authority,)> = LazyLock::new(|| (Authority::from_static(ADDRESS),));

        let (links,) = &*LINKS;

        if ![None, Some(&Scheme::HTTPS), Some(&Scheme::HTTP)].contains(&link.scheme()) {
            return;
        }

        if link.authority() != Some(links) {
            return;
        }

        let path = link.path();

        match path {
            "/" => self.home = false,
            "/next" => self.next = false,
            "/prev" => self.prev = false,
            _ => {}
        }
    }
}

impl Display for MissingLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Your webpage is missing the following links:")?;
        if self.home {
            writeln!(f, "- {ADDRESS}")?;
        }
        if self.next {
            writeln!(f, "- {ADDRESS}/next")?;
        }
        if self.prev {
            writeln!(f, "- {ADDRESS}/prev")?;
        }

        writeln!(
            f,
            "\nWhat to do:
- If your webpage is rendered client-side, ask the administrators to set the validator to only check for your site being online.
- If you don't use anchor tags with href attributes for the links, add the attribute `data-phwebring=\"prev\"|\"home\"|\"next\"` to the link elements.
- If you think this alert is in error, contact Henry."
        )
    }
}

impl Error for MissingLinks {}

async fn contains_link(
    webpage: impl Stream<Item = reqwest::Result<Bytes>> + Unpin,
) -> Option<MissingLinks> {
    let mut reader = Reader::from_reader(StreamReader::new(
        webpage.map_err(|e| std::io::Error::new(ErrorKind::Other, e)),
    ));

    let decoder = reader.decoder();

    let mut missing_links = MissingLinks {
        home: true,
        next: true,
        prev: true,
    };

    let mut buf = Vec::new();

    while let Ok(event) = reader.read_event_into_async(&mut buf).await {
        if let Event::Start(tag) = event {
            if tag.name().0 == b"a" {
                let Ok(Some(attr)) = tag.try_get_attribute("href") else {
                    continue;
                };

                let Ok(attr_value) = attr.decode_and_unescape_value(decoder) else {
                    continue;
                };

                let Ok(uri) = attr_value.parse::<Uri>() else {
                    continue;
                };

                missing_links.found_link(&uri);
            }

            let Ok(Some(attr)) = tag.try_get_attribute("data-phwebring") else {
                continue;
            };

            let Ok(attr_value) = attr.decode_and_unescape_value(decoder) else {
                continue;
            };

            match &*attr_value {
                "prev" => missing_links.prev = false,
                "home" => missing_links.home = false,
                "next" => missing_links.next = false,
                _ => {}
            }
        }

        if !missing_links.home && !missing_links.next && !missing_links.prev {
            return None;
        }
    }

    Some(missing_links)
}
