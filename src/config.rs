//! Configuration file handling

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::http::Uri;
use eyre::Context;
use log::LevelFilter;
use reqwest::Url;
use sarlacc::Intern;
use serde::{Deserialize, Deserializer, de};

/// Webring configuration object
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub webring: WebringTable,
    pub network: NetworkTable,
    #[serde(default = "default_logging_table")]
    pub logging: LoggingTable,
    pub discord: Option<DiscordTable>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WebringTable {
    /// Directory from which to serve static content
    pub static_dir: PathBuf,

    /// File to read member database from
    pub members_file: PathBuf,

    /// Base URL of the webring, e.g. `https://ring.purduehackers.com`
    #[serde(
        default = "default_address",
        deserialize_with = "deserialize_interned_uri"
    )]
    pub base_url: Intern<Uri>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NetworkTable {
    /// Address/port to listen on
    pub listen_addr: SocketAddr,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LoggingTable {
    /// Log verbosity
    #[serde(with = "LevelFilterWrapper")]
    pub verbosity: LevelFilter,

    /// File to print logs to in addition to the console
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DiscordTable {
    /// Discord webhook URL
    #[serde(deserialize_with = "deserialize_url")]
    pub webhook_url: Url,
}

// This type exists so serde can figure out what variants are available for the verbosity option.
// If we use LevelFilter directly, it uses the Display and FromStr implementations, which means
// there isn't a list of possible variants for clap to use.
#[derive(Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase", remote = "LevelFilter")]
enum LevelFilterWrapper {
    Off,
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LevelFilterWrapper> for LevelFilter {
    fn from(val: LevelFilterWrapper) -> Self {
        match val {
            LevelFilterWrapper::Off => LevelFilter::Off,
            LevelFilterWrapper::Trace => LevelFilter::Trace,
            LevelFilterWrapper::Debug => LevelFilter::Debug,
            LevelFilterWrapper::Info => LevelFilter::Info,
            LevelFilterWrapper::Warn => LevelFilter::Warn,
            LevelFilterWrapper::Error => LevelFilter::Error,
        }
    }
}

/// Get default webring base address.
fn default_address() -> Intern<Uri> {
    Intern::new(Uri::from_static(env!("CARGO_PKG_HOMEPAGE")))
}

/// Get default log level.
fn default_logging_table() -> LoggingTable {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    LoggingTable {
        verbosity: level,
        log_file: None,
    }
}

// Deserialize an `Intern<Uri>` from a string value
fn deserialize_interned_uri<'de, D>(deserializer: D) -> Result<Intern<Uri>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let uri = Uri::try_from(s).map_err(de::Error::custom)?;
    if uri.authority().is_none() {
        return Err(de::Error::custom(
            "URL does not have a host/authority component",
        ));
    }
    Ok(Intern::new(uri))
}

// Deserialize an `Option<Url>` from a string value
fn deserialize_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    Url::parse(<&str>::deserialize(deserializer)?).map_err(de::Error::custom)
}

impl Config {
    /// Load configuration from the given TOML file.
    pub async fn parse_from_file(path: &Path) -> eyre::Result<Self> {
        let file_contents = tokio::fs::read_to_string(path).await?;
        toml::from_str(&file_contents).wrap_err("Failed to load configuration file")
    }
}
