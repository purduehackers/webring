// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use axum::http::Uri;
use chrono::FixedOffset;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    last_seen: u64,
    started_from: Arc<Uri>,
    most_recently_at: Arc<Uri>,
}

#[derive(Serialize, Deserialize)]
struct AggregatedStats {
    // Date → (From, To) → Started From → Count
    // graph:
    //     DashMap<DateTime<FixedOffset>, DashMap<(Arc<Uri>, Arc<Uri>), DashMap<Arc<Uri>, AtomicU64>>>,
}

#[derive(Clone, Debug)]
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
