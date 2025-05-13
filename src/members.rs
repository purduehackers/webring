use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use rand::seq::SliceRandom;

use log::warn;

#[allow(clippy::unused_async)]
async fn check(website: &str, check_level: CheckLevel) -> Result<bool, ()> {
    // TODO
    Ok(true)
}

enum CheckLevel {
    ForLinks,
    Online,
    None,
}

struct Member {
    website: Arc<str>,
    check_level: CheckLevel,
    check_successful: AtomicBool,
}

struct MembersData {
    members_table: HashMap<String, usize>,
    ordering: Vec<Member>,
}

#[derive(Clone)]
pub struct Members {
    // https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    // This is a good case for std locks
    inner: Arc<RwLock<MembersData>>,
}

impl Members {
    pub fn next_page(&self, name: &str) -> Option<Arc<str>> {
        let inner = self.inner.read().unwrap();
        let mut idx = *inner.members_table.get(name)?;

        for _ in 0..inner.ordering.len() {
            idx += 1;
            if idx == inner.ordering.len() {
                idx = 0;
            }

            if inner.ordering[idx].check_successful.load(Ordering::Relaxed) {
                return Some(Arc::clone(&inner.ordering[idx].website));
            }
        }

        warn!("All webring members are broken???");

        None
    }

    pub fn prev_page(&self, name: &str) -> Option<Arc<str>> {
        let inner = self.inner.read().unwrap();
        let mut idx = *inner.members_table.get(name)?;

        for _ in 0..inner.ordering.len() {
            if idx == 0 {
                idx = inner.ordering.len();
            }
            idx -= 1;

            if inner.ordering[idx].check_successful.load(Ordering::Relaxed) {
                return Some(Arc::clone(&inner.ordering[idx].website));
            }
        }

        warn!("All webring members are broken???");

        None
    }

    pub fn random_page(&self) -> Option<Arc<str>> {
        let inner = self.inner.read().unwrap();
        let mut range = (0..inner.ordering.len()).collect::<Vec<_>>();
        let mut range = &mut *range;

        let mut rng = rand::rng();

        while !range.is_empty() {
            let (chosen, rest) = range.partial_shuffle(&mut rng, 1);
            let chosen = chosen[0];

            if inner.ordering[chosen]
                .check_successful
                .load(Ordering::Relaxed)
            {
                return Some(Arc::clone(&inner.ordering[chosen].website));
            }

            range = rest;
        }

        warn!("All webring members are broken???");

        None
    }
}
