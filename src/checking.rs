use std::{error::Error, fmt::Display};

use axum::http::Uri;
use chrono::Utc;
use eyre::Report;
use log::{error, info};
use tokio::sync::RwLock;

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

            let text = res.text().await?;

            match contains_link(&text) {
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

impl Display for MissingLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "The webpage is missing the following links:")?;
        if self.home {
            writeln!(f, "- {ADDRESS}")?;
        }
        if self.next {
            writeln!(f, "- {ADDRESS}/next")?;
        }
        if self.prev {
            writeln!(f, "- {ADDRESS}/prev")?;
        }

        Ok(())
    }
}

impl Error for MissingLinks {}

fn contains_link(webpage: &str) -> Option<MissingLinks> {
    None
}
