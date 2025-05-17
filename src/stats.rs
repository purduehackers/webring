use std::{net::IpAddr, path::PathBuf, sync::Arc};

use axum::http::Uri;
use chrono::{DateTime, FixedOffset};
use dashmap::DashMap;

const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    last_seen: u64,
    started_from: Arc<Uri>,
    most_recently_at: Arc<Uri>,
}

#[derive(Clone, Debug)]
struct AggregatedStats {}

#[derive(Clone, Debug)]
pub struct Stats {
    aggregated: DashMap<DateTime<FixedOffset>, AggregatedStats>,
    data: DashMap<IpAddr, IpInfo>,
}

impl Stats {
    pub async fn new(stats_file: PathBuf) -> eyre::Result<Stats> {
        Ok(Stats {
            data: DashMap::new(),
            aggregated: DashMap::new(),
        })
    }
}
