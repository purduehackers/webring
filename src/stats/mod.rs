// TODO: Everything in this file
#![allow(dead_code)]
#![allow(unused_variables)]

use std::{
    net::IpAddr,
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use log::info;
use papaya::HashMap;
use sarlacc::Intern;

const IP_TRACKING_TTL: chrono::TimeDelta = Duration::days(1);
const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

pub static UNKNOWN_ORIGIN: LazyLock<Intern<str>> = LazyLock::new(|| Intern::from_ref("unknown"));

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

#[derive(Debug, Default)]
pub struct Stats {
    aggregated: AggregatedStats,
    ip_tracking: HashMap<IpAddr, IpInfo>,
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            aggregated: AggregatedStats::default(),
            ip_tracking: HashMap::new(),
        }
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

    pub fn prune_seen_ips(&self) {
        self.prune_seen_ips_impl(Utc::now());
    }

    #[expect(clippy::cast_possible_wrap)]
    fn prune_seen_ips_impl(&self, now: DateTime<Utc>) {
        let mut ip_tracking = self.ip_tracking.pin();
        // Since papaya is lock-free it may be possible for things to be inserted in between these before/after calls so the number may not be exact. Too bad!
        let before = ip_tracking.len();
        ip_tracking.retain(|_ip_addr, info| now - info.last_seen < IP_TRACKING_TTL);
        let after = ip_tracking.len();
        info!("Pruned ~{} IP addresses", after as i64 - before as i64);
    }
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, sync::atomic::Ordering};

    use chrono::{DateTime, Duration, NaiveDate, Utc};
    use sarlacc::Intern;

    use crate::stats::IP_TRACKING_TTL;

    use super::{Stats, TIMEZONE};

    fn a(addr: &str) -> IpAddr {
        addr.parse().unwrap()
    }

    fn i(str: &str) -> Intern<str> {
        Intern::from_ref(str)
    }

    fn t(timestamp: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(timestamp * 1000).unwrap()
    }

    fn d(timestamp: i64) -> NaiveDate {
        t(timestamp).with_timezone(&TIMEZONE).date_naive()
    }

    impl Stats {
        fn g(&self, timestamp: i64, from: &str, to: &str, started_from: &str) -> u64 {
            self.aggregated
                .counters
                .pin()
                .get(&(d(timestamp), i(from), i(to), i(started_from)))
                .unwrap()
                .load(Ordering::Relaxed)
        }
    }

    #[tokio::test]
    async fn test_stat_tracking() {
        let stats = Stats::new();

        stats.redirected_impl(a("0.0.0.0"), i("a.com"), i("b.com"), t(0));
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1));

        stats.redirected_impl(a("1.0.0.0"), i("a.com"), i("b.com"), t(1));
        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("homepage.com"), t(2));
        stats.redirected_impl(a("1.0.0.0"), i("homepage.com"), i("c.com"), t(3));

        assert_eq!(stats.aggregated.counters.len(), 4);
        assert_eq!(stats.g(0, "a.com", "b.com", "a.com"), 2);
        assert_eq!(stats.g(0, "b.com", "c.com", "a.com"), 1);
        assert_eq!(stats.g(0, "b.com", "homepage.com", "a.com"), 1);
        assert_eq!(stats.g(0, "homepage.com", "c.com", "a.com"), 1);

        let tracking = stats.ip_tracking.pin();
        assert_eq!(tracking.len(), 2);

        let zero = tracking.get(&a("0.0.0.0")).unwrap();
        assert_eq!(zero.last_seen, t(1));
        assert_eq!(zero.started_from, i("a.com"));

        let one = tracking.get(&a("1.0.0.0")).unwrap();
        assert_eq!(one.last_seen, t(3));
        assert_eq!(one.started_from, i("a.com"));

        stats.prune_seen_ips_impl(t(1000));

        assert_eq!(tracking.len(), 2);

        let zero = tracking.get(&a("0.0.0.0")).unwrap();
        assert_eq!(zero.last_seen, t(1));
        assert_eq!(zero.started_from, i("a.com"));

        let one = tracking.get(&a("1.0.0.0")).unwrap();
        assert_eq!(one.last_seen, t(3));
        assert_eq!(one.started_from, i("a.com"));

        stats.prune_seen_ips_impl(t(1) + IP_TRACKING_TTL);

        assert_eq!(tracking.len(), 1);

        let one = tracking.get(&a("1.0.0.0")).unwrap();
        assert_eq!(one.last_seen, t(3));
        assert_eq!(one.started_from, i("a.com"));

        let day = Duration::days(1);

        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1) + day);
        stats.redirected_impl(a("0.0.0.0"), i("c.com"), i("a.com"), t(2) + day);

        assert_eq!(stats.aggregated.counters.len(), 6);
        assert_eq!(stats.g(0, "a.com", "b.com", "a.com"), 2);
        assert_eq!(stats.g(0, "b.com", "c.com", "a.com"), 1);
        assert_eq!(stats.g(0, "b.com", "homepage.com", "a.com"), 1);
        assert_eq!(stats.g(0, "homepage.com", "c.com", "a.com"), 1);
        assert_eq!(stats.g(day.num_seconds(), "b.com", "c.com", "b.com"), 1);
        assert_eq!(stats.g(day.num_seconds(), "c.com", "a.com", "b.com"), 1);
    }
}
