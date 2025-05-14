use std::sync::atomic::{AtomicI64, Ordering};

use axum::http::Uri;
use chrono::Utc;
use log::{error, info};
use tokio::sync::RwLock;

use crate::webring::CheckLevel;

static LAST_PING: AtomicI64 = AtomicI64::new(i64::MIN);
static PING_SUCCESSFUL: RwLock<bool> = RwLock::const_new(false);

async fn is_online() -> bool {
    let now = Utc::now().timestamp_millis();
    let last_ping = LAST_PING.load(Ordering::Acquire);
    if now < last_ping + 1000 {
        return *PING_SUCCESSFUL.read().await;
    }

    let mut result = PING_SUCCESSFUL.write().await;

    let now = Utc::now().timestamp_millis();
    let last_ping = LAST_PING.load(Ordering::Acquire);
    if now < last_ping + 1000 {
        return *result;
    }

    let status = surge_ping::ping("8.8.8.8".parse().unwrap(), &[0; 8]).await;

    *result = status.is_ok();

    let now = Utc::now().timestamp_millis();
    LAST_PING.store(now, Ordering::Release);

    *result
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

            todo!()
        }
        CheckLevel::JustOnline => {
            reqwest::get(website.to_string())
                .await?
                .error_for_status()?;

            Ok(())
        }
        CheckLevel::None => Ok(()),
    }
}
