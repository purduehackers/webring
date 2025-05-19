// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

use std::{net::IpAddr, path::PathBuf, sync::atomic::AtomicU64};

use chrono::{DateTime, FixedOffset};
use dashmap::DashMap;
use sarlacc::Intern;
use serde::{Deserialize, Serialize};

const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    last_seen: u64,
    started_from: Intern<str>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Graph {
    // (From, To) → Started From → Count
    #[expect(clippy::type_complexity)]
    graph: DashMap<(Intern<str>, Intern<str>), DashMap<Intern<str>, AtomicU64>>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct AggregatedStats {
    // Date → Graph
    time_series: DashMap<DateTime<FixedOffset>, Graph>,
}

#[derive(Debug, Default)]
pub struct Stats {
    aggregated: AggregatedStats,
    data: DashMap<IpAddr, IpInfo>,
}

impl Stats {
    #[expect(clippy::unused_async)]
    pub async fn new(stats_file: PathBuf) -> eyre::Result<Stats> {
        Ok(Stats {
            data: DashMap::new(),
            aggregated: AggregatedStats {
                time_series: DashMap::new(),
            },
        })
    }
}
