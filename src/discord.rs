//! Send messages to members over Discord
//!
//! Uses [Discord's webhook feature][webhook-api-ref].
//!
//! [webhook-api-ref]: https://discord.com/developers/docs/resources/webhook#execute-webhook

use std::{fmt::Display, str::FromStr, time::Duration};

use eyre::{Context, bail};
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{self, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A user agent representing our program. [Required by Discord][api-doc-ua].
///
/// [api-doc-ua]: https://discord.com/developers/docs/reference#user-agent
const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ", ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// Represents a snowflake, the type used to encode identifiers in Discord APIs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Snowflake(u64);
impl Snowflake {
    /// Create a new snowflake from the given value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}
impl From<u64> for Snowflake {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
impl From<Snowflake> for u64 {
    fn from(value: Snowflake) -> Self {
        value.0
    }
}
impl FromStr for Snowflake {
    type Err = <u64 as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}
impl Display for Snowflake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A notifier that sends messages to a Discord channel using a webhook URL.
#[derive(Clone, Debug)]
pub struct DiscordNotifier {
    /// The webhook URL to send messages to.
    webhook_url: reqwest::Url,
    /// HTTP client used to send requests.
    client: Client,
}

impl DiscordNotifier {
    /// Create a new notifier that sends notifications using the given
    /// [webhook](https://discord.com/developers/docs/resources/webhook#execute-webhook) URL.
    pub fn new(webhook_url: &Url) -> Self {
        Self {
            webhook_url: webhook_url.to_owned(),
            client: Client::new(),
        }
    }

    /// Send a message in the channel this notifier is registered to.
    pub async fn send_message(&self, ping: Option<Snowflake>, message: &str) -> eyre::Result<()> {
        loop {
            let response = self.send_single_message(ping, message).await?;
            // If we get rate limited, try again.
            // See https://discord.com/developers/docs/topics/rate-limits.
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                match response.headers().get(header::RETRY_AFTER) {
                    Some(value) => {
                        sleep_from_retry_after(value).await?;
                        continue;
                    }
                    None => bail!("Got rate-limited but response contains no Retry-After header"),
                }
            }
            response
                .error_for_status()
                .wrap_err("Discord webhook returned error")?;
            break;
        }
        Ok(())
    }

    /// Sends a single message to the Discord webhook.
    ///
    /// Returns the HTTP [`Response`] from the API.
    ///
    /// # Errors
    ///
    /// On request error, returns an `Err` with the error details, but with the URL stripped, so it
    /// can safely be logged or displayed to a user.
    async fn send_single_message(
        &self,
        ping: Option<Snowflake>,
        message: &str,
    ) -> eyre::Result<Response> {
        const SUPPRESS_EMBEDS: u16 = 1 << 2;
        self.client
            .post(self.webhook_url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .json(&json!(
                {
                    "content": message,
                    "allowed_mentions": match ping {
                        Some(id) => json!({ "users": [id.to_string()] }),
                        None => json!({}),
                    },
                    "flags": SUPPRESS_EMBEDS,
                }
            ))
            .query(&[("wait", "true")])
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .wrap_err("Failed to execute Discord webhook")
    }
}

/// Sleep for the number of seconds specified in the `Retry-After` HTTP header of a response.
///
/// # Errors
///
/// Returns an `Err` if the `Retry-After` value cannot be parsed.
async fn sleep_from_retry_after(header_val: &HeaderValue) -> eyre::Result<()> {
    let val_str = header_val
        .to_str()
        .wrap_err("Retry-After header is invalid")?;
    let seconds = val_str
        .parse::<u64>()
        .wrap_err("Retry-After header cannot be parsed into a u64")?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    use super::DiscordNotifier;

    /// Tests sending a message in Discord.
    ///
    /// Requires the following environment variables:
    /// - `DISCORD_WEBHOOK_URL`: The Discord webhook URL to send test messages to.
    /// - `DISCORD_PING_USER_ID`: The ID of the user to ping.
    #[tokio::test]
    #[ignore = "requires secret Discord webhook environment"]
    async fn send_notification() {
        let url = std::env::var("DISCORD_WEBHOOK_URL").unwrap();
        let user_id = std::env::var("DISCORD_PING_USER_ID").unwrap();
        let notifier = DiscordNotifier::new(&Url::parse(&url).unwrap());
        // Send a message with no ping
        notifier.send_message(None, "this is a test").await.unwrap();
        // Send a message with a ping
        notifier
            .send_message(
                Some(user_id.parse().unwrap()),
                &format!("this is a test to ping <@{user_id}>"),
            )
            .await
            .unwrap();
    }
}
