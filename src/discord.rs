//! Send messages to members over Discord
//!
//! Uses [Discord's webhook feature][webhook-api-ref].
//!
//! [webhook-api-ref]: https://discord.com/developers/docs/resources/webhook#execute-webhook

use std::{
    collections::HashMap,
    fmt::Display,
    str::FromStr,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use eyre::{Context, bail};
use reqwest::{
    Client, Response, StatusCode, Url,
    header::{self, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Maximum number of attempts to send a message before giving up.
const MAX_DELIVERY_ATTEMPTS: usize = 5;

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
#[derive(Debug)]
pub struct DiscordNotifier {
    /// The webhook URL to send messages to.
    webhook_url: reqwest::Url,
    /// HTTP client used to send requests.
    client: Client,
    /// Set of pending messages to be sent.
    /// Map from recipient -> (message, attempt count).
    message_queue: Mutex<HashMap<Option<Snowflake>, (String, usize)>>,
}

impl DiscordNotifier {
    /// Create a new notifier that sends notifications using the given
    /// [webhook](https://discord.com/developers/docs/resources/webhook#execute-webhook) URL.
    pub fn new(webhook_url: &Url) -> Self {
        Self {
            webhook_url: webhook_url.to_owned(),
            client: Client::new(),
            message_queue: Mutex::new(HashMap::new()),
        }
    }

    /// Lock the message queue, recovering from a poisoned lock if necessary.
    fn lock_message_queue(&self) -> MutexGuard<'_, HashMap<Option<Snowflake>, (String, usize)>> {
        // If the lock is poisoned, we clear it and return a new lock.
        self.message_queue.lock().unwrap_or_else(|mut err| {
            log::error!("Discord notification queue was poisoned; some notifications may be lost");
            **err.get_mut() = HashMap::new();
            self.message_queue.clear_poison();
            err.into_inner()
        })
    }

    /// Enqueue a message to be sent to the Discord channel the next time notifications are
    /// dispatched.
    ///
    /// Only one message is sent per user per notification dispatch, so if a message was already
    /// enqueued with the same `ping` value, it will be replaced with the new message.
    pub fn enqueue_message(&self, ping: Option<Snowflake>, message: String) {
        self.lock_message_queue().insert(ping, (message, 0));
    }

    /// Dispatch all messages in the queue, sending them to Discord.
    ///
    /// Returns `(n_sent, n_failed)` where `n_sent` is the number of messages that were sent
    /// successfully, and `n_failed` is the number of messages that failed to send. Errors are
    /// logged but not returned.
    pub async fn dispatch_messages(&self) -> (usize, usize) {
        // Take the map to avoid race conditions while sending messages.
        let mut map = {
            let mut lock = self.lock_message_queue();
            std::mem::take(&mut *lock)
        };
        let mut sent = 0;
        let mut failed = 0;
        for (ping, (message, attempts)) in map.drain() {
            if attempts >= MAX_DELIVERY_ATTEMPTS {
                log::error!(
                    "Failed to send message to {} after {attempts} attempts; giving up",
                    ping.map_or("channel".to_string(), |id| id.to_string())
                );
                continue;
            }
            if let Err(err) = self.send_message(ping, &message).await {
                log::error!("Failed to send Discord message: {err}");
                // If no new message was enqueued for the same recipient, retry this one.
                let mut lock = self.lock_message_queue();
                if lock.get(&ping).is_none() {
                    lock.insert(ping, (message, attempts + 1));
                }
                failed += 1;
            } else {
                sent += 1;
            }
        }
        (sent, failed)
    }

    /// Send a message in the channel this notifier is registered to.
    async fn send_message(&self, ping: Option<Snowflake>, message: &str) -> eyre::Result<()> {
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
    use httpmock::{Method, MockServer};
    use reqwest::Url;

    use super::{DiscordNotifier, MAX_DELIVERY_ATTEMPTS};

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

    /// Tests that messages destined for the same user are deduplicated
    #[tokio::test]
    async fn test_dispatch_deduplicate() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(Method::POST);
            then.status(200);
        });
        let notifier = DiscordNotifier::new(&Url::parse(&server.base_url()).unwrap());

        notifier.enqueue_message(Some(1234.into()), "first message".to_string());
        // Next message should replace the first one
        notifier.enqueue_message(Some(1234.into()), "second message".to_string());
        notifier.enqueue_message(Some(4321.into()), "message for other user".to_string());
        notifier.enqueue_message(None, "message for no user".to_string());

        let (sent, failed) = notifier.dispatch_messages().await;

        assert_eq!(3, sent);
        assert_eq!(0, failed);
        assert_eq!(3, mock.hits_async().await);

        // Ensure sent messages were removed
        let (sent, failed) = notifier.dispatch_messages().await;
        assert_eq!(0, sent);
        assert_eq!(0, failed);
        assert_eq!(3, mock.hits_async().await); // 3 were sent last round
    }

    #[tokio::test]
    async fn test_give_up_after_max_attempts() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(Method::POST);
            then.status(500);
        });
        let notifier = DiscordNotifier::new(&Url::parse(&server.base_url()).unwrap());

        notifier.enqueue_message(Some(1234.into()), "test message".to_string());

        // Expect MAX_DELIVERY_ATTEMPTS attempts
        for i in 0..MAX_DELIVERY_ATTEMPTS {
            let (sent, failed) = notifier.dispatch_messages().await;
            assert_eq!(0, sent);
            assert_eq!(1, failed);
            assert_eq!(i + 1, mock.hits_async().await);
        }

        // Expect no more attempts
        let (sent, failed) = notifier.dispatch_messages().await;
        assert_eq!(0, sent);
        assert_eq!(0, failed);
        assert_eq!(MAX_DELIVERY_ATTEMPTS, mock.hits_async().await);
    }
}
