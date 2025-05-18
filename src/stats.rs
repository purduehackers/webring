// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

use std::{net::IpAddr, path::PathBuf, sync::atomic::AtomicU64};

use axum::http::{Uri, uri::InvalidUri};
use chrono::{DateTime, FixedOffset};
use dashmap::DashMap;
use sarlacc::Intern;
use serde::{Deserialize, Serialize};

const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    last_seen: u64,
    started_from: Intern<Uri>,
    most_recently_at: Intern<Uri>,
}

#[derive(PartialEq, Eq, Hash, Clone, Copy, Serialize, Deserialize)]
#[serde(into = "String", try_from = "&str")]
struct UriWrapper(Intern<Uri>);

impl From<UriWrapper> for String {
    fn from(val: UriWrapper) -> Self {
        val.0.to_string()
    }
}

impl TryFrom<&str> for UriWrapper {
    type Error = InvalidUri;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse::<Uri>().map(|v| UriWrapper(Intern::new(v)))
    }
}

#[derive(Serialize, Deserialize)]
struct AggregatedStats {
    // Date → (From, To) → Started From → Count
    graph: DashMap<
        DateTime<FixedOffset>,
        DashMap<(UriWrapper, UriWrapper), DashMap<UriWrapper, AtomicU64>>,
    >,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    // aggregated: AggregatedStats,
    data: DashMap<IpAddr, IpInfo>,
}

impl Stats {
    #[expect(clippy::unused_async)]
    pub async fn new(stats_file: PathBuf) -> eyre::Result<Stats> {
        Ok(Stats {
            data: DashMap::new(),
            // aggregated: AggregatedStats {
            //     graph: DashMap::new(),
            // },
        })
    }
}
