// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

mod parquet_encoding;

use std::{
    fs::File,
    io::ErrorKind,
    net::IpAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use eyre::Report;
use log::error;
use papaya::HashMap;
use parquet_encoding::{decode_parquet, encode_parquet};
use sarlacc::Intern;
use tokio::{select, sync::watch};

const IP_TRACKING_TTL: chrono::TimeDelta = Duration::days(1);
const PARQUET_STATS_TTL: chrono::TimeDelta = Duration::minutes(1);
const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    last_seen: DateTime<Utc>,
    started_from: Intern<str>,
}

#[derive(Debug, Default)]
struct AggregatedStats {
    // (Date (with timezone `TIMEZONE`), From, To, Started From) → Count
    #[expect(clippy::type_complexity)]
    counters: HashMap<(NaiveDate, Intern<str>, Intern<str>, Intern<str>), AtomicU64>,
}

#[derive(Debug)]
pub struct Stats {
    aggregated: Arc<AggregatedStats>,
    ip_tracking: HashMap<IpAddr, IpInfo>,
    parquet_file: watch::Receiver<Option<Arc<[u8]>>>,
}

impl Default for Stats {
    fn default() -> Self {
        let (_, rx) = watch::channel(None);

        Self {
            aggregated: Arc::new(AggregatedStats::default()),
            ip_tracking: HashMap::default(),
            parquet_file: rx,
        }
    }
}

impl Stats {
    pub async fn new(stats_file: PathBuf) -> eyre::Result<Stats> {
        let decoded = match File::open(&stats_file) {
            Ok(file) => tokio::task::spawn_blocking(move || decode_parquet(file))
                .await
                .unwrap()?,
            Err(e) => match e.kind() {
                ErrorKind::NotFound => AggregatedStats {
                    counters: HashMap::new(),
                },
                _ => return Err(Report::from(e)),
            },
        };

        let aggregated = Arc::new(decoded);

        let (tx, rx) = watch::channel(None);
        Self::spawn_parquet_task(Arc::clone(&aggregated), tx, stats_file);

        Ok(Self::new_from_data(aggregated, rx))
    }

    fn new_from_data(
        aggregated: Arc<AggregatedStats>,
        rx: watch::Receiver<Option<Arc<[u8]>>>,
    ) -> Stats {
        Stats {
            ip_tracking: HashMap::new(),
            parquet_file: rx,
            aggregated,
        }
    }

    fn spawn_parquet_task(
        aggregated: Arc<AggregatedStats>,
        tx: watch::Sender<Option<Arc<[u8]>>>,
        stats_file: PathBuf,
    ) {
        tokio::spawn(async move {
            let ttl_duration = PARQUET_STATS_TTL.to_std().unwrap();

            loop {
                let sleep_task = tokio::time::sleep(ttl_duration);

                let aggregated_for_task = Arc::clone(&aggregated);
                match tokio::task::spawn_blocking(move || encode_parquet(&aggregated_for_task))
                    .await
                    .unwrap()
                {
                    Ok(v) => {
                        if let Err(e) = tokio::fs::write(&stats_file, &v).await {
                            error!("{e}");
                        }

                        if tx.send(Some(Arc::from(v))).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        error!("{e}");
                        if tx.send(None).is_err() {
                            return;
                        }
                    }
                }

                select! {
                    () = sleep_task => {}
                    () = tx.closed() => return
                };
            }
        });
    }

    pub fn redirected(&self, ip: IpAddr, from: Intern<str>, to: Intern<str>) {
        self.redirected_impl(ip, from, to, Utc::now());
    }

    fn redirected_impl(&self, ip: IpAddr, from: Intern<str>, to: Intern<str>, now: DateTime<Utc>) {
        let ip_info = *self.ip_tracking.pin().update_or_insert(
            ip,
            |ip_info| {
                let mut info = *ip_info;
                info.last_seen = now;
                info
            },
            IpInfo {
                last_seen: now,
                started_from: from,
            },
        );

        let date = now.with_timezone(&TIMEZONE).date_naive();

        let pinned_map = self.aggregated.counters.pin();
        let counter =
            pinned_map.get_or_insert((date, from, to, ip_info.started_from), AtomicU64::new(0));

        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn prune_seen_ips(&self) {
        self.prune_seen_ips_impl(Utc::now());
    }

    fn prune_seen_ips_impl(&self, now: DateTime<Utc>) {
        self.ip_tracking
            .pin()
            .retain(|_ip_addr, info| now - info.last_seen > IP_TRACKING_TTL);
    }

    fn get_parquet(&self) -> Option<Arc<[u8]>> {
        (*self.parquet_file.borrow()).as_ref().map(Arc::clone)
    }
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, sync::Arc};

    use axum::body::Bytes;
    use chrono::{DateTime, Utc};
    use papaya::HashMap;
    use sarlacc::Intern;
    use tokio::sync::watch;

    use crate::stats::{
        AggregatedStats,
        parquet_encoding::{decode_parquet, encode_parquet},
    };

    use super::Stats;

    fn a(addr: &str) -> IpAddr {
        addr.parse().unwrap()
    }

    fn i(str: &str) -> Intern<str> {
        Intern::from_ref(str)
    }

    fn t(timestamp: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(timestamp * 1000).unwrap()
    }

    #[tokio::test]
    async fn test_stat_tracking() {
        let (_, rx) = watch::channel(None);
        let stats = Stats::new_from_data(
            Arc::new(AggregatedStats {
                counters: HashMap::new(),
            }),
            rx,
        );

        stats.redirected_impl(a("0.0.0.0"), i("a.com"), i("b.com"), t(0));
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1));

        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("c.com"), t(0));
        stats.redirected_impl(a("1.0.0.0"), i("c.com"), i("b.com"), t(1));
        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("a.com"), t(2));

        let parquet = encode_parquet(&stats.aggregated).unwrap();
        let decoded = decode_parquet(Bytes::from(Box::from(&*parquet))).unwrap();
    }
}
