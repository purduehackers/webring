/*
Copyright (C) 2025 Kian Kasad and Henry Rovnyak

This file is part of the Purdue Hackers webring.

The Purdue Hackers webring is free software: you can redistribute it and/or
modify it under the terms of the GNU Affero General Public License as
published by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

The Purdue Hackers webring is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License
for more details.

You should have received a copy of the GNU Affero General Public License along
with the Purdue Hackers webring. If not, see <https://www.gnu.org/licenses/>.
*/

//! Configuration file handling

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::http::Uri;
use indexmap::IndexMap;
use reqwest::Url;
use sarlacc::Intern;
use serde::{Deserialize, Deserializer, de};
use tracing::{instrument, level_filters::LevelFilter};

use crate::{
    discord::Snowflake,
    webring::{CheckLevel, EnrollmentStatus},
};

/// Webring configuration object
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    /// Ring configuration
    pub webring: WebringTable,
    /// Network/server configuration
    pub network: NetworkTable,
    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingTable,
    /// Discord integration configuration
    pub discord: Option<DiscordTable>,

    /// Map from member name to their site details
    #[serde(default)]
    pub members: IndexMap<String, MemberSpec>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WebringTable {
    /// Directory from which to serve static content
    pub static_dir: PathBuf,

    /// Base URL of the webring, e.g. `https://ring.purduehackers.com`
    ///
    /// It is guaranteed to have a valid host/authority component
    #[serde(
        default = "default_address",
        deserialize_with = "deserialize_interned_uri"
    )]
    base_url: Intern<Uri>,
}

impl WebringTable {
    /// Gets the base URL of the webring
    pub fn base_url(&self) -> Intern<Uri> {
        self.base_url
    }
}

/// Network/server configuration table
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NetworkTable {
    /// Address/port to listen on
    pub listen_addr: SocketAddr,
}

/// Logging configuration table
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LoggingTable {
    /// Log verbosity
    #[serde(alias = "level", deserialize_with = "deserialize_level_filter")]
    pub verbosity: LevelFilter,

    /// File to print logs to in addition to the console
    pub log_file: Option<PathBuf>,

    /// Sentry.io DSN to connect to for Sentry integration
    pub sentry_dsn: Option<sentry::types::Dsn>,
}

impl Default for LoggingTable {
    fn default() -> Self {
        Self {
            verbosity: if cfg!(debug_assertions) {
                LevelFilter::DEBUG
            } else {
                LevelFilter::INFO
            },
            log_file: None,
            sentry_dsn: None,
        }
    }
}

/// Discord integration configuration table
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DiscordTable {
    /// Discord webhook URL
    #[serde(deserialize_with = "deserialize_url")]
    pub webhook_url: Url,
}

/// Describes a webring member
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MemberSpec {
    /// URI of the member's site
    ///
    /// It is guaranteed to have a non-empty authority component.
    #[serde(
        alias = "url",
        alias = "site",
        deserialize_with = "deserialize_interned_uri"
    )]
    uri: Intern<Uri>,
    /// Discord ID of the member, if they opt in to Discord integration
    pub discord_id: Option<Snowflake>,
    /// Level of checks to perform on the member's site
    #[serde(default)]
    pub check_level: CheckLevel,
    /// Enrollment status of the member
    #[serde(default)]
    pub enrollment: EnrollmentStatus,
}

impl MemberSpec {
    /// Get the member's URI
    pub fn uri(&self) -> Intern<Uri> {
        self.uri
    }
}

/// This type exists so serde can figure out what variants are available for the verbosity option.
/// If we use [`LevelFilter`] directly, it uses the [`Display`][std::fmt::Display] and
/// [`FromStr`][std::str::FromStr] implementations, which means there isn't a list of possible
/// variants for clap to use.
#[derive(Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LevelFilterWrapper {
    /// No logging
    Off,
    /// Log everything
    Trace,
    /// Log debug messages and higher
    Debug,
    /// Log informational messages and higher
    Info,
    /// Log warnings and higher
    Warn,
    /// Log errors only
    Error,
}

impl From<LevelFilterWrapper> for LevelFilter {
    fn from(val: LevelFilterWrapper) -> Self {
        match val {
            LevelFilterWrapper::Off => LevelFilter::OFF,
            LevelFilterWrapper::Trace => LevelFilter::TRACE,
            LevelFilterWrapper::Debug => LevelFilter::DEBUG,
            LevelFilterWrapper::Info => LevelFilter::INFO,
            LevelFilterWrapper::Warn => LevelFilter::WARN,
            LevelFilterWrapper::Error => LevelFilter::ERROR,
        }
    }
}

/// Deserialize a [`LevelFilter`] by deserializing a [`LevelFilterWrapper`] first and converting it
/// using [`From<LevelFilterWrapper>`].
fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    LevelFilterWrapper::deserialize(deserializer).map(LevelFilter::from)
}

/// Get default webring base address.
fn default_address() -> Intern<Uri> {
    Intern::new(Uri::from_static(env!("CARGO_PKG_HOMEPAGE")))
}

/// Deserialize an `Intern<Uri>` from a string value
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

/// Deserialize an `Option<Url>` from a string value
fn deserialize_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    Url::parse(&String::deserialize(deserializer)?).map_err(de::Error::custom)
}

impl Config {
    /// Load configuration from a TOML string.
    pub fn parse_from_toml(toml: &str) -> eyre::Result<Self> {
        Ok(toml::from_str(toml)?)
    }

    /// Load configuration from the given TOML file.
    #[instrument(err)]
    pub async fn parse_from_file(path: &Path) -> eyre::Result<Self> {
        let file_contents = tokio::fs::read_to_string(path).await?;
        Self::parse_from_toml(&file_contents)
    }

    /// Returns `true` if any settings other than members have changed between the old and new
    /// configurations.
    pub fn diff_settings(old: &Config, new: &Config) -> bool {
        old.webring != new.webring
            || old.network != new.network
            || old.logging != new.logging
            || old.discord != new.discord
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::http::Uri;
    use indexmap::IndexMap;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use reqwest::Url;
    use sarlacc::Intern;
    use tracing::level_filters::LevelFilter;

    use crate::{
        config::MemberSpec,
        discord::Snowflake,
        webring::{CheckLevel, EnrollmentStatus},
    };

    use super::{Config, DiscordTable, LoggingTable, NetworkTable, WebringTable};

    #[test]
    fn valid_config() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            base-url = "https://ring.purduehackers.com"

            [network]
            listen-addr = "0.0.0.0:3000"

            [logging]
            verbosity = "info"
            sentry-dsn = "https://c0f6872ae63b8b24101faf62177bbd0b@o2398742983742982.ingest.us.sentry.io/2734298742987340"

            [discord]
            webhook-url = "https://api.discord.com/webhook-or-something"
        "# };
        let actual: Config =
            toml::from_str(config).expect("Expected parsing configuration to succeed");
        let expected = Config {
            webring: WebringTable {
                static_dir: PathBuf::from("static"),
                base_url: Intern::new(Uri::from_static("https://ring.purduehackers.com/")),
            },
            network: NetworkTable {
                listen_addr: "0.0.0.0:3000".parse().unwrap(),
            },
            logging: LoggingTable {
                verbosity: LevelFilter::INFO,
                log_file: None,
                sentry_dsn: Some("https://c0f6872ae63b8b24101faf62177bbd0b@o2398742983742982.ingest.us.sentry.io/2734298742987340".parse().unwrap()),
            },
            discord: Some(DiscordTable {
                webhook_url: Url::parse("https://api.discord.com/webhook-or-something").unwrap(),
            }),
            members: IndexMap::new(),
        };
        assert_eq!(expected, actual);
    }

    #[test]
    fn default_base_url() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            listen-addr = "0.0.0.0:3000"
        "# };
        let actual: Config = toml::from_str(config).unwrap();
        assert_eq!(
            "https://ring.purduehackers.com/",
            &actual.webring.base_url.to_string()
        );
    }

    #[test]
    fn missing_optional_section() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            listen-addr = "0.0.0.0:3000"
        "# };
        let actual: Config = toml::from_str(config).unwrap();
        assert_eq!(None, actual.discord);
    }

    #[test]
    fn missing_default_section() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            listen-addr = "0.0.0.0:3000"
        "# };
        let actual: Config = toml::from_str(config).unwrap();
        assert_eq!(
            LoggingTable {
                verbosity: if cfg!(debug_assertions) {
                    LevelFilter::DEBUG
                } else {
                    LevelFilter::INFO
                },
                log_file: None,
                sentry_dsn: None,
            },
            actual.logging
        );
    }

    #[test]
    fn missing_required_field() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            [discord]
            webhook-url = "https://api.discord.com/webhook-or-something"
        "# };
        let result = toml::from_str::<Config>(config);
        assert!(result.is_err());
        assert_eq!("missing field `listen-addr`", result.unwrap_err().message());
    }

    #[test]
    fn missing_required_section() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [discord]
            webhook-url = "https://api.discord.com/webhook-or-something"
        "# };
        let result = toml::from_str::<Config>(config);
        assert!(result.is_err());
        assert_eq!("missing field `network`", result.unwrap_err().message());
    }

    #[test]
    fn extra_key() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            extra-field = 123
            [network]
            listen-addr = "0.0.0.0:3000"
            [discord]
            webhook-url = "https://api.discord.com/webhook-or-something"
        "# };
        let result = toml::from_str::<Config>(config);
        assert!(result.is_err());
        assert_eq!(
            "unknown field `extra-field`, expected `static-dir` or `base-url`",
            result.unwrap_err().message()
        );
    }

    #[test]
    fn members_as_objects() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            listen-addr = "0.0.0.0:3000"
            [members]
            kian = { url = "https://kasad.com", discord-id = 123456789 }
            henry = { url = "https://hrovnyak.gitlab.io", check-level = "none", enrollment = "alum" }
        "# };
        let result = toml::from_str::<Config>(config).unwrap();
        assert_eq!(2, result.members.len());
        assert_eq!(
            MemberSpec {
                uri: Intern::new(Uri::from_static("https://kasad.com")),
                discord_id: Some(Snowflake::from(123_456_789)),
                check_level: CheckLevel::ForLinks,
                enrollment: EnrollmentStatus::Student,
            },
            result.members["kian"]
        );
        assert_eq!(
            MemberSpec {
                uri: Intern::new(Uri::from_static("https://hrovnyak.gitlab.io")),
                discord_id: None,
                check_level: CheckLevel::None,
                enrollment: EnrollmentStatus::Alum,
            },
            result.members["henry"]
        );
    }

    #[test]
    fn members_as_tables() {
        let config = indoc! { r#"
            [webring]
            static-dir = "static"
            [network]
            listen-addr = "0.0.0.0:3000"

            [members.kian]
            url = "https://kasad.com"
            discord-id = 123456789

            [members.henry]
            url = "https://hrovnyak.gitlab.io"
            check-level = "none"
        "# };
        let result = toml::from_str::<Config>(config).unwrap();
        assert_eq!(2, result.members.len());
        assert_eq!(
            MemberSpec {
                uri: Intern::new(Uri::from_static("https://kasad.com")),
                discord_id: Some(Snowflake::from(123_456_789)),
                check_level: CheckLevel::ForLinks,
                enrollment: EnrollmentStatus::default(),
            },
            result.members["kian"]
        );
        assert_eq!(
            MemberSpec {
                uri: Intern::new(Uri::from_static("https://hrovnyak.gitlab.io")),
                discord_id: None,
                check_level: CheckLevel::None,
                enrollment: EnrollmentStatus::default(),
            },
            result.members["henry"]
        );
    }

    #[test]
    fn preserve_member_order() {
        let members: IndexMap<String, MemberSpec> = toml::from_str(indoc! { r#"
            c = { url = "c.com" }
            b = { url = "b.com" }
            a = { url = "a.com" }
        "# })
        .unwrap();
        let list = members.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>();
        assert_eq!(vec!["c", "b", "a"], list);
    }
}
