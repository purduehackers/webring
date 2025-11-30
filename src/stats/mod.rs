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

use core::fmt::Debug;
use std::{
    io::Write,
    net::IpAddr,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering},
    },
};

use axum::http::uri::Authority;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use papaya::{Guard, HashMap};
use sarlacc::Intern;
use seize::Collector;
use tracing::{error, info, instrument};

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
struct AggregatedStats<W: Write + Send + 'static> {
    /// The collector for the atomic data that we're handling
    collector: Arc<Collector>,
    /// The last date that a redirect was tracked (hopefully today)
    today: AtomicI32,
    /// (From, To, Started From) → Count
    /// Invariant: This MUST ALWAYS be a valid pointer
    counter: AtomicPtr<Counts>,
    /// The writer for the statistics output file
    output: Arc<Mutex<W>>,
}

impl<W: Write + Send + 'static> Debug for AggregatedStats<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregatedStats")
            .field("today", &self.today.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Convert the current time into the days since the epoch
fn mk_num(time: DateTime<Utc>) -> i32 {
    time.date_naive().to_epoch_days()
}

impl<W: Write + Send + 'static> AggregatedStats<W> {
    /// Create a new `AggregatedStats`
    fn new(now: DateTime<Utc>, writer: W) -> Self {
        let counter: Counts = HashMap::default();

        AggregatedStats {
            today: AtomicI32::new(mk_num(now)),
            counter: AtomicPtr::new(Box::into_raw(Box::new(counter))),
            collector: Arc::new(Collector::new()),
            output: Arc::new(Mutex::new(writer)),
        }
    }

    /// Retrieve the current counter from a guard
    fn counter<'a>(&'a self, guard: &'a impl Guard) -> &'a Counts {
        // SAFETY: The counter is guaranteed to be a valid pointer and we are using Acquire ordering to synchronize-with its initialization
        unsafe { &*guard.protect(&self.counter, Ordering::Acquire) }
    }

    /// Retrieve the current counter from a guard while updating it if the current time is a new calendar date
    fn maybe_update_counter<'a>(&'a self, now: DateTime<Utc>, guard: &'a impl Guard) -> &'a Counts {
        let now = mk_num(now);

        let mut prev_day = self.today.load(Ordering::Relaxed);

        // If our "now" time is in the past relative to "today" (perhaps tasks got out of order or something), we want to count this redirect towards the most recent day rather than replacing and writing the data.

        while prev_day < now {
            match self
                .today
                .compare_exchange(prev_day, now, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(new_prev_day) => {
                    prev_day = new_prev_day;
                }
            }
        }

        if prev_day < now {
            let new_counter: *mut Counts = Box::into_raw(Box::new(HashMap::new()));

            // Release to synchronize-with `counter` and Acquire to ensure that we can see the initialization of the previous one so that we can properly read it.
            //
            // We do not need to guard the `prev_ptr` because it will not be retired by any other tasks in the meantime. Proof:
            // 1. The only code path where we could retire a pointer stored in this location is this one, and this code path is guaranteed to retire the value of `prev_ptr`.
            // 2. The only way for our `prev_ptr` to get retired while we are holding it is for another task in this code branch to also hold an identical `prev_ptr` at the same time as us.
            // 3. Suppose for contradiction that there exists a task $x$ that is different from this one $y$ such that $x$'s `prev_ptr` equals our `prev_ptr`.
            // 4. `prev_ptr` is valid by the invariant on `self.counter`.
            // 5. For all tasks $t∈T$ on this code path, its value of `new_counter` is valid at the same time that its `prev_ptr` is valid, because they both need to be valid when `swap` is performed by the invariant.
            // 6. $x$'s and $y$'s `prev_ptr` is valid during all swaps between $x$'s and $y$'s inclusive in `self.counter`'s total order, since we suppose that $x$ is still holding `prev_ptr` at least until $y$ calls `swap`.
            // 7. For any task $s∈S⊆T$ where $s$'s call to `swap` is between $x$'s and $y$'s inclusive in the total order, by (5) and (6), $s$'s `new_counter` is valid at the same time that `prev_ptr` is.
            // 8. By correctness of the memory allocator and (7), $s$'s `new_counter` != `prev_ptr`.
            // 9. The task previous to this one wrote its `new_counter` to `self.counter`, which by (8) does not equal `prev_ptr`
            // 10. That value is stored in `prev_ptr` in $y$ due to it being swapped out.
            // 11. `prev_ptr` != `prev_ptr`. This is a contradiction, proving that the situation is impossible and we do not need to guard `prev_ptr`.
            //
            // Why not just guard it anyways for safety? Because we would need to transfer the guard into our `task::spawn_blocking`, and since the lifetime of a guard is tied to a lifetime of a collector, and since <https://blog.polybdenum.com/2024/06/07/the-inconceivable-types-of-rust-how-to-make-self-borrows-safe.html> isn't implemented, we cannot move the guard into the task, therefore we cannot guard `prev_ptr`.
            let prev_ptr = self.counter.swap(new_counter, Ordering::AcqRel);

            let output = Arc::clone(&self.output);

            // Allow it to be moved into our task
            let prev_ptr = prev_ptr as usize;

            let this_collector = Arc::clone(&self.collector);

            tokio::task::spawn_blocking(move || {
                let mut output = output.lock().unwrap();

                let prev_ptr = prev_ptr as *mut Counts;
                // SAFETY: Since this pointer hasn't been retired yet, we have access to it until we do retire it.
                let prev = unsafe { &*prev_ptr }.pin();

                for ((from, to, started_from), count) in &prev {
                    let count = count.load(Ordering::Relaxed);
                    if let Err(e) = output.write_fmt(format_args!(
                        "{prev_day},{from},{to},{started_from},{count}\n"
                    )) {
                        error!("Error writing statistics: {e}");
                    }
                }

                // SAFETY: The pointer can no longer be accessed from a new location since we previously overwrote the atomic pointer, and `Box::from_raw` is the correct way to drop the pointer. This task is also finished with its access to it.
                unsafe { this_collector.retire(prev_ptr, |ptr, _| drop(Box::from_raw(ptr))) }
            });
        }

        self.counter(guard)
    }
}

/// Statistics tracking for the webring
pub struct Stats<W: Write + Send + 'static> {
    /// Aggregated statistics
    aggregated: AggregatedStats<W>,
    /// Map of IP information keyed by IP address
    ip_tracking: HashMap<IpAddr, IpInfo>,
}

impl<W: Write + Send + 'static> Debug for Stats<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stats")
            .field("aggregated", &self.aggregated)
            .field("ip_tracking", &self.ip_tracking)
            .finish()
    }
}

impl<W: Write + Send> Stats<W> {
    /// Creates a new instance of `Stats`.
    pub fn new(now: DateTime<Utc>, writer: W) -> Stats<W> {
        Stats {
            aggregated: AggregatedStats::new(now, writer),
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
    use std::{collections::HashSet, net::IpAddr, str::from_utf8};

    use axum::http::uri::Authority;
    use chrono::{DateTime, Duration, NaiveDate, Utc};
    use indoc::indoc;
    use sarlacc::Intern;
    use tokio::sync::mpsc::{self, UnboundedReceiver};

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

    struct TestWriter(mpsc::UnboundedSender<Vec<u8>>);

    impl std::io::Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.send(buf.to_owned()).unwrap();
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    async fn assert_same_data(rx: &mut UnboundedReceiver<Vec<u8>>, expected: &str) {
        let mut data = Vec::new();
        while data.len() != expected.len() {
            data.extend(rx.recv().await.unwrap());
        }
        assert_eq!(
            from_utf8(&data)
                .unwrap()
                .split('\n')
                .collect::<HashSet<_>>(),
            expected.split('\n').collect::<HashSet<_>>()
        );
    }

    #[tokio::test]
    async fn test_stat_tracking() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stats = Stats::new(t(0), TestWriter(tx));

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

        assert!(rx.is_empty());
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(1) + day);
        assert_same_data(
            &mut rx,
            indoc! {"
            0,a.com,b.com,a.com,2
            0,b.com,c.com,a.com,1
            0,b.com,homepage.com,a.com,1
            0,homepage.com,c.com,a.com,1
        "},
        )
        .await;
        stats.redirected_impl(a("0.0.0.0"), i("b.com"), i("c.com"), t(4));
        stats.redirected_impl(a("0.0.0.0"), i("c.com"), i("a.com"), t(2) + day);

        assert_eq!(stats.aggregated.counter(&guard).len(), 2);
        stats.assert_stat_entry(("b.com", "c.com", "b.com"), 2);
        stats.assert_stat_entry(("c.com", "a.com", "b.com"), 1);

        stats.redirected_impl(a("0.0.0.0"), i("c.com"), i("a.com"), t(2) + day + day);
        assert_same_data(
            &mut rx,
            indoc! {"
            1,b.com,c.com,b.com,2
            1,c.com,a.com,b.com,1
        "},
        )
        .await;

        assert_eq!(stats.aggregated.counter(&guard).len(), 1);
        stats.assert_stat_entry(("c.com", "a.com", "b.com"), 1);
    }
}
