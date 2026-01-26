/*
Copyright (C) 2025 Henry Rovnyak

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

//! Statistics collection and processing

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

use axum::http::uri::Authority;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use papaya::HashMap;
use sarlacc::Intern;
use tracing::{info, instrument};

/// The TTL for IP tracking entries, after which they are considered stale and removed.
const IP_TRACKING_TTL: chrono::TimeDelta = Duration::days(1);
/// The time zone of the webring.
pub const TIMEZONE: chrono::FixedOffset = FixedOffset::west_opt(5 * 3600).unwrap();

/// Placeholder origin to use when the origin is unknown.
pub static UNKNOWN_ORIGIN: LazyLock<Intern<Authority>> =
    LazyLock::new(|| Intern::new("unknown".parse().unwrap()));

/// Information about a client IP address
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IpInfo {
    /// The last time this IP address was seen
    last_seen: DateTime<Utc>,
    /// The authority this IP address started from
    started_from: Intern<Authority>,
}

/// Counters for click statistic groups
#[derive(Debug, Default)]
struct AggregatedStats {
    /// (Date (with timezone `TIMEZONE`), From, To, Started From) → Count
    #[expect(clippy::type_complexity)]
    counters: HashMap<
        (
            NaiveDate,
            Intern<Authority>,
            Intern<Authority>,
            Intern<Authority>,
        ),
        AtomicU64,
    >,
}

/// Tracks statistics of clicks through the webring
#[derive(Debug, Default)]
pub struct Stats {
    /// Aggregated statistics
    aggregated: AggregatedStats,
    /// Map of IP information keyed by IP address
    ip_tracking: HashMap<IpAddr, IpInfo>,
}

impl Stats {
    /// Creates a new instance of `Stats`.
    pub fn new() -> Stats {
        Stats {
            aggregated: AggregatedStats::default(),
            ip_tracking: HashMap::new(),
        }
    }

    /// Records a redirect event from one authority to another for a given client IP address.
    pub fn redirected(&self, ip: IpAddr, from: Intern<Authority>, to: Intern<Authority>) {
        self.redirected_impl(ip, from, to, Utc::now());
    }

    /// Records a redirect event from one authority to another for a given client IP address at a specific time.
    ///
    /// Split apart to allow injecting the current time for testing purposes.
    fn redirected_impl(
        &self,
        ip: IpAddr,
        from: Intern<Authority>,
        to: Intern<Authority>,
        now: DateTime<Utc>,
    ) {
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

    /// Prunes stale IP addresses from the tracking map.
    #[instrument(name = "stats.prune_seen_ips", skip(self))]
    pub fn prune_seen_ips(&self) {
        self.prune_seen_ips_impl(Utc::now());
    }

    /// Implementation of the IP pruning logic.
    ///
    /// Split apart to allow injecting the current time for testing purposes.
    #[expect(clippy::cast_possible_wrap)]
    fn prune_seen_ips_impl(&self, now: DateTime<Utc>) {
        let mut ip_tracking = self.ip_tracking.pin();
        // Since papaya is lock-free it may be possible for things to be inserted in between these before/after calls so the number may not be exact. Too bad!
        let before = ip_tracking.len();
        ip_tracking.retain(|_ip_addr, info| now - info.last_seen < IP_TRACKING_TTL);
        let after = ip_tracking.len();
        info!(count = after as i64 - before as i64, "Pruned IP addresses");
    }

    #[cfg(test)]
    pub fn assert_stat_entry(&self, entry: (NaiveDate, &str, &str, &str), count: u64) {
        assert_eq!(
            self.aggregated
                .counters
                .pin()
                .get(&(
                    entry.0,
                    Intern::new(entry.1.parse::<Authority>().unwrap()),
                    Intern::new(entry.2.parse::<Authority>().unwrap()),
                    Intern::new(entry.3.parse::<Authority>().unwrap()),
                ))
                .map_or(0, |v| v.load(Ordering::Relaxed)),
            count,
            "{self:#?}\n{entry:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use axum::http::uri::Authority;
    use chrono::{DateTime, Duration, NaiveDate, Utc};
    use sarlacc::Intern;

    use crate::stats::IP_TRACKING_TTL;

    use super::{Stats, TIMEZONE};

    fn a(addr: &str) -> IpAddr {
        addr.parse().unwrap()
    }

    fn i(str: &str) -> Intern<Authority> {
        Intern::new(str.parse().unwrap())
    }

    fn t(timestamp: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(timestamp * 1000).unwrap()
    }

    fn d(timestamp: i64) -> NaiveDate {
        t(timestamp).with_timezone(&TIMEZONE).date_naive()
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
        stats.assert_stat_entry((d(0), "a.com", "b.com", "a.com"), 2);
        stats.assert_stat_entry((d(0), "b.com", "c.com", "a.com"), 1);
        stats.assert_stat_entry((d(0), "b.com", "homepage.com", "a.com"), 1);
        stats.assert_stat_entry((d(0), "homepage.com", "c.com", "a.com"), 1);

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
        stats.assert_stat_entry((d(0), "a.com", "b.com", "a.com"), 2);
        stats.assert_stat_entry((d(0), "b.com", "c.com", "a.com"), 1);
        stats.assert_stat_entry((d(0), "b.com", "homepage.com", "a.com"), 1);
        stats.assert_stat_entry((d(0), "homepage.com", "c.com", "a.com"), 1);
        stats.assert_stat_entry((d(day.num_seconds()), "b.com", "c.com", "b.com"), 1);
        stats.assert_stat_entry((d(day.num_seconds()), "b.com", "c.com", "b.com"), 1);
        stats.assert_stat_entry((d(day.num_seconds()), "c.com", "a.com", "b.com"), 1);
    }
}
