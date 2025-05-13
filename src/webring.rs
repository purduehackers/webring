use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::http::{Uri, uri::Authority};
use eyre::eyre;
use futures::{StreamExt, stream::FuturesUnordered};
use rand::seq::SliceRandom;

use log::warn;

use crate::render_homepage::Homepage;

#[allow(clippy::unused_async)]
async fn check(website: &Uri, check_level: CheckLevel) -> eyre::Result<bool> {
    // TODO
    Ok(true)
}

#[derive(Clone, Copy, Debug)]
enum CheckLevel {
    ForLinks,
    JustOnline,
    None,
}

#[derive(Debug)]
struct Member {
    name: String,
    website: Arc<Uri>,
    discord_id: String,
    check_level: CheckLevel,
    check_successful: Arc<AtomicBool>,
}

impl Member {
    fn check_and_store(&self) -> impl Future<Output = eyre::Result<()>> + Send + 'static {
        let website = Arc::clone(&self.website);
        let check_level = self.check_level;
        let successful = Arc::clone(&self.check_successful);

        async move {
            successful.store(check(&website, check_level).await?, Ordering::Relaxed);

            Ok(())
        }
    }
}

#[derive(Debug)]
struct WebringData {
    members_table: HashMap<Authority, usize>,
    ordering: Vec<Member>,
}

/// The data structure underlying a webring. Implements the core webring functionality.
#[derive(Clone)]
pub struct Webring {
    // https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    // This is a good case for std locks because we will never need to hold it across an await
    inner: Arc<RwLock<WebringData>>,
    // This one we would like to hold across awaits
    homepage: Arc<tokio::sync::RwLock<Option<Arc<Homepage>>>>,
    path: Arc<Path>,
}

impl Webring {
    /// Create a webring by parsing the file at the given path.
    pub async fn new(file: PathBuf) -> eyre::Result<Webring> {
        let webring_data = parse_file(&file).await?;
        let webring = Webring {
            inner: Arc::new(RwLock::new(webring_data)),
            path: Arc::from(file),
            homepage: Arc::new(tokio::sync::RwLock::new(None)),
        };

        webring.check_members().await?;

        Ok(webring)
    }

    /// Update the webring in-place by re-parsing the file given in the original `new` call, and invalidating the cache of the SSR'ed homepage.
    ///
    /// Useful for updating the webring without restarting the server.
    pub async fn update_from_file(&self) -> eyre::Result<()> {
        let mut new_members = parse_file(&self.path).await?;
        let tasks = FuturesUnordered::new();

        {
            let old_members = self.inner.read().unwrap();

            for (name, idx) in &new_members.members_table {
                match old_members.members_table.get(name) {
                    Some(old_idx) => {
                        let check_successful =
                            Arc::clone(&old_members.ordering[*old_idx].check_successful);
                        new_members.ordering[*idx].check_successful = check_successful;
                    }
                    None => {
                        tasks.push(new_members.ordering[*idx].check_and_store());
                    }
                }
            }
        }

        let ret = collect_errs(tasks).await;

        *self.homepage.write().await = None;

        ret
    }

    /// Query everyone's webpages and check them according to their respective check levels.
    pub async fn check_members(&self) -> eyre::Result<()> {
        let tasks = FuturesUnordered::new();

        {
            let inner = self.inner.read().unwrap();

            for member in &inner.ordering {
                tasks.push(member.check_and_store());
            }
        }

        let ret = collect_errs(tasks).await;

        *self.homepage.write().await = None;

        ret
    }

    /// Get the next page in the webring from the given URI; based on the authority part
    ///
    /// Returns None if the URI has no authority (is relative) or all of the webring members failed the check.
    pub fn next_page(&self, uri: &Uri) -> Option<Arc<Uri>> {
        let inner = self.inner.read().unwrap();
        let mut idx = *inner.members_table.get(uri.authority()?)?;

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

    /// Get the previous page in the webring from the given URI; based on the authority part
    ///
    /// Returns None if the URI has no authority (is relative) or all of the webring members failed the check.
    pub fn prev_page(&self, uri: &Uri) -> Option<Arc<Uri>> {
        let inner = self.inner.read().unwrap();
        let mut idx = *inner.members_table.get(uri.authority()?)?;

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

    /// Get a random page in the webring.
    ///
    /// Returns None if all of the webring members failed the check.
    pub fn random_page(&self) -> Option<Arc<Uri>> {
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

    /// Return a server-side rendered homepage.
    ///
    /// This method is cached, and the cache gets invalidated whenever the webring is updated.
    pub async fn homepage(&self) -> eyre::Result<Arc<Homepage>> {
        let maybe_homepage = self.homepage.read().await;

        if let Some(homepage) = &*maybe_homepage {
            Ok(Arc::clone(homepage))
        } else {
            drop(maybe_homepage);
            let mut maybe_homepage = self.homepage.write().await;

            // Just in case it got written between releasing and acquiring the lock
            if let Some(homepage) = &*maybe_homepage {
                return Ok(Arc::clone(homepage));
            }

            let members = {
                let inner = self.inner.read().unwrap();

                inner
                    .ordering
                    .iter()
                    .map(|member_info| MemberForHomepage {
                        name: member_info.name.clone(),
                        website: (*member_info.website).clone(),
                        check_successful: member_info.check_successful.load(Ordering::Relaxed),
                    })
                    .collect::<Vec<_>>()
            };

            let homepage = Arc::new(Homepage::new(&members).await?);

            *maybe_homepage = Some(Arc::clone(&homepage));

            Ok(homepage)
        }
    }
}

pub struct MemberForHomepage {
    pub name: String,
    pub website: Uri,
    pub check_successful: bool,
}

async fn collect_errs(
    tasks: FuturesUnordered<impl Future<Output = Result<(), eyre::Error>> + Send>,
) -> Result<(), eyre::Error> {
    let errs = tasks
        .filter_map(async |res| res.err())
        .collect::<Vec<_>>()
        .await;

    if errs.is_empty() {
        Ok(())
    } else {
        Err(eyre!(
            errs.into_iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n\n")
        ))
    }
}

async fn parse_file(path: &Path) -> eyre::Result<WebringData> {
    let file = tokio::fs::read_to_string(&path).await?;

    let mut members = WebringData {
        members_table: HashMap::new(),
        ordering: Vec::new(),
    };

    for line in file.lines().filter(|line| !line.is_empty()) {
        let split = line.split("—").map(str::trim).collect::<Vec<_>>();

        if split.len() != 4 {
            return Err(eyre!(
                "Expected four parameters of the form `name — website — discord id — check level`. Got:\n\n{line}{}",
                if line.contains(['-', '–']) {
                    "\n\nHelp: Dashes are expected to be emdashes (—) to avoid clashing with regular dashes."
                } else {
                    ""
                }
            ));
        }

        #[allow(clippy::match_on_vec_items)]
        let check_level = match &*split[3].to_lowercase() {
            "for links" => CheckLevel::ForLinks,
            "just online" => CheckLevel::JustOnline,
            "none" => CheckLevel::None,
            _ => {
                return Err(eyre!(
                    "Expected the check level to be one of {{\"for links\", \"just online\", \"none\"}}. Got:\n\n{line}"
                ));
            }
        };

        let uri = split[1].parse::<Uri>()?;

        let authority = match uri.authority() {
            Some(v) => v.to_owned(),
            None => return Err(eyre!("URLs must not be relative. Got: {uri}")),
        };

        let member = Member {
            name: split[0].to_owned(),
            website: Arc::from(uri),
            discord_id: split[2].to_owned(),
            check_level,
            check_successful: Arc::new(AtomicBool::new(false)),
        };

        members
            .members_table
            .insert(authority, members.ordering.len());
        members.ordering.push(member);
    }

    Ok(members)
}
