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
        atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering},
    },
};

use axum::http::uri::Authority;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use papaya::{Guard, HashMap};
use sarlacc::Intern;
use seize::Collector;
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

type Counts = HashMap<(Intern<Authority>, Intern<Authority>, Intern<Authority>), AtomicU64>;

/// The counts for all of the possible webring redirects
#[derive(Debug)]
struct AggregatedStats {
    /// The collector for the atomic data that we're handling
    collector: Collector,
    /// The last date that a redirect was tracked (hopefully today)
    today: AtomicI32,
    /// (From, To, Started From) → Count
    /// Invariant: This MUST ALWAYS be a valid pointer
    counter: AtomicPtr<Counts>,
}

impl AggregatedStats {
    /// Create a new `AggregatedStats`
    fn new(now: DateTime<Utc>) -> Self {
        let counter: Counts = HashMap::default();

        AggregatedStats {
            today: AtomicI32::new(Self::mk_num(now)),
            counter: AtomicPtr::new(Box::into_raw(Box::new(counter))),
            collector: Collector::new(),
        }
    }

    /// Convert the current time into the days since the epoch
    fn mk_num(time: DateTime<Utc>) -> i32 {
        time.date_naive().to_epoch_days()
    }

    /// Retrieve the current counter from a guard
    fn counter<'a>(&'a self, guard: &'a impl Guard) -> &'a Counts {
        // SAFETY: The counter is guaranteed to be a valid pointer and we are using Acquire ordering to synchronize-with its initialization
        unsafe { &*guard.protect(&self.counter, Ordering::Acquire) }
    }

    /// Retrieve the current counter from a guard while updating it if the current time is a new calendar date
    fn maybe_update_counter<'a>(&'a self, now: DateTime<Utc>, guard: &'a impl Guard) -> &'a Counts {
        let now = AggregatedStats::mk_num(now);

        let prev_day = self.today.swap(now, Ordering::Relaxed);

        if prev_day != now {
            let new_counter: *mut Counts = Box::into_raw(Box::new(HashMap::new()));

            // Release to synchronize-with `counter`. We don't need Acquire because we won't read the previous pointer.
            let prev = guard.swap(&self.counter, new_counter, Ordering::Release);
            // SAFETY: The pointer can no longer be accessed now that it has been swapped into `prev`, and `Box::from_raw` is the correct way to drop the pointer.
            unsafe {
                self.collector
                    .retire(prev, |ptr, _| drop(Box::from_raw(ptr)));
            }
        }

        self.counter(guard)
    }
}

/// Statistics tracking for the webring
#[derive(Debug)]
pub struct Stats {
    /// Aggregated statistics
    aggregated: AggregatedStats,
    /// Map of IP information keyed by IP address
    ip_tracking: HashMap<IpAddr, IpInfo>,
}

impl Stats {
    /// Creates a new instance of `Stats`.
    pub fn new(now: DateTime<Utc>) -> Stats {
        Stats {
            aggregated: AggregatedStats::new(now),
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

        let guard = self.aggregated.collector.enter();
        let pinned_map = self.aggregated.maybe_update_counter(now, &guard).pin();
        let counter = pinned_map.get_or_insert((from, to, ip_info.started_from), AtomicU64::new(0));

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
    pub fn assert_stat_entry(&self, entry: (&str, &str, &str), count: u64) {
        let guard = self.aggregated.collector.enter();
        assert_eq!(
            self.aggregated
                .counter(&guard)
                .pin()
                .get(&(
                    Intern::new(entry.0.parse::<Authority>().unwrap()),
                    Intern::new(entry.1.parse::<Authority>().unwrap()),
                    Intern::new(entry.2.parse::<Authority>().unwrap()),
                ),)
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
        let stats = Stats::new(t(0));

        stats.redirected_impl(a("0.0.0.0"), i("a.com"), i("b.com"), t(0));
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1));

        stats.redirected_impl(a("1.0.0.0"), i("a.com"), i("b.com"), t(1));
        stats.redirected_impl(a("1.0.0.0"), i("b.com"), i("homepage.com"), t(2));
        stats.redirected_impl(a("1.0.0.0"), i("homepage.com"), i("c.com"), t(3));

        let guard = stats.aggregated.collector.enter();
        assert_eq!(stats.aggregated.counter(&guard).len(), 4);
        stats.assert_stat_entry(("a.com", "b.com", "a.com"), 2);
        stats.assert_stat_entry(("b.com", "c.com", "a.com"), 1);
        stats.assert_stat_entry(("b.com", "homepage.com", "a.com"), 1);
        stats.assert_stat_entry(("homepage.com", "c.com", "a.com"), 1);

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

        assert_eq!(stats.aggregated.counter(&guard).len(), 2);
        stats.assert_stat_entry(("b.com", "c.com", "b.com"), 1);
        stats.assert_stat_entry(("c.com", "a.com", "b.com"), 1);
    }
}
