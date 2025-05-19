// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

mod parquet_encoding;

use std::{
    net::IpAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use log::error;
use papaya::HashMap;
use parquet_encoding::encode_parquet;
use sarlacc::Intern;
use tokio::sync::{mpsc, oneshot};

const IP_TRACKING_TTL: chrono::TimeDelta = Duration::days(1);
const JSON_STATS_TTL: chrono::TimeDelta = Duration::minutes(1);
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

type JSONCacheMessage = (
    oneshot::Sender<parquet::errors::Result<Arc<[u8]>>>,
    DateTime<Utc>,
);

#[derive(Debug, Default)]
pub struct Stats {
    aggregated: Arc<AggregatedStats>,
    ip_tracking: HashMap<IpAddr, IpInfo>,
    json_cache_channel: mpsc::Sender<JSONCacheMessage>,
}

impl Stats {
    #[expect(clippy::unused_async)]
    pub async fn new(stats_file: PathBuf) -> eyre::Result<Stats> {
        Ok(Self::new_empty())
    }

    fn new_empty() -> Stats {
        let aggregated = Arc::new(AggregatedStats {
            counters: HashMap::new(),
        });

        Stats {
            ip_tracking: HashMap::new(),
            json_cache_channel: Self::spawn_json_task(Arc::clone(&aggregated)),
            aggregated,
        }
    }

    fn spawn_json_task(aggregated: Arc<AggregatedStats>) -> mpsc::Sender<JSONCacheMessage> {
        let (tx, mut rx) = mpsc::channel::<JSONCacheMessage>(256);

        tokio::spawn(async move {
            let mut cache = None;
            let mut last_computed = DateTime::<Utc>::MIN_UTC;

            while let Some((json_tx, now)) = rx.recv().await {
                if now - last_computed > JSON_STATS_TTL {
                    cache = None;
                }

                let data = if let Some(data) = &cache {
                    Arc::clone(data)
                } else {
                    last_computed = now;
                    let aggregated_for_task = Arc::clone(&aggregated);
                    let new_data = match tokio::task::spawn_blocking(move || {
                        encode_parquet(&aggregated_for_task)
                    })
                    .await
                    .unwrap()
                    {
                        Ok(v) => Arc::from(v),
                        Err(e) => {
                            error!("{e}");
                            // Dropping the oneshot is a bug
                            json_tx.send(Err(e)).unwrap();
                            continue;
                        }
                    };

                    cache = Some(Arc::clone(&new_data));

                    new_data
                };

                // Dropping the oneshot is a bug
                json_tx.send(Ok(data)).unwrap();
            }
        });

        tx
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

    async fn get_json(&self) -> parquet::errors::Result<Arc<[u8]>> {
        self.get_json_impl(Utc::now()).await
    }

    async fn get_json_impl(&self, now: DateTime<Utc>) -> parquet::errors::Result<Arc<[u8]>> {
        let (tx, rx) = oneshot::channel();

        // It is a bug if the task exits early so unwrapping the channels is OK
        self.json_cache_channel.send((tx, now)).await.unwrap();
        rx.await.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use chrono::{DateTime, Utc};
    use sarlacc::Intern;

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
        let stats = Stats::new_empty();

        stats.redirected_impl(a("0.0.0.0"), i("a.com"), i("b.com"), t(0));
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1));

        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("c.com"), t(0));
        stats.redirected_impl(a("1.0.0.0"), i("c.com"), i("b.com"), t(1));
        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("a.com"), t(2));

        panic!("{:?}", stats.get_json_impl(t(3)).await.unwrap().len());
    }
}
