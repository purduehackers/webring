use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc, OnceLock, RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::http::{Uri, uri::Authority};
use eyre::{Context, bail, eyre};
use futures::{StreamExt, stream::FuturesUnordered};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rand::seq::SliceRandom;

use log::{debug, error, info, warn};
use thiserror::Error;

use crate::{
    checking::{CheckFailure, check},
    discord::{DiscordNotifier, Snowflake},
    homepage::{Homepage, MemberForHomepage},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckLevel {
    None,
    JustOnline,
    ForLinks,
}

impl FromStr for CheckLevel {
    type Err = eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "links" => CheckLevel::ForLinks,
            "online" => CheckLevel::JustOnline,
            "none" => CheckLevel::None,
            _ => {
                bail!(
                    "Expected the check level to be one of {{\"links\", \"online\", \"none\"}}. Got \"{s}\""
                );
            }
        })
    }
}

#[derive(Clone, Debug)]
struct Member {
    name: String,
    website: Arc<Uri>,
    discord_id: Option<Snowflake>,
    check_level: CheckLevel,
    check_successful: Arc<AtomicBool>,
}

impl Member {
    fn check_and_store_and_optionally_notify(
        &self,
        base_address: &Uri,
        notifier: Option<Arc<DiscordNotifier>>,
    ) -> impl Future<Output = Option<CheckFailure>> + Send + 'static {
        let website = Arc::clone(&self.website);
        let check_level = self.check_level;
        let successful = Arc::clone(&self.check_successful);

        let discord_id_for_block = self.discord_id;
        let base_address_for_block = base_address.clone();
        async move {
            let check_result = check(&website, check_level, &base_address_for_block).await;
            if let Some(failure) = &check_result {
                successful.store(false, Ordering::Relaxed);
                if let (Some(notifier), Some(user_id)) = (notifier, discord_id_for_block) {
                    let message = failure.to_message();
                    tokio::spawn(async move {
                        notifier
                            .send_message(Some(user_id), &message)
                            .await
                            .unwrap();
                    });
                }
            } else {
                successful.store(true, Ordering::Relaxed);
            }
            check_result
        }
    }
}

#[derive(Clone, Debug, Default)]
struct WebringData {
    members_table: HashMap<Authority, usize>,
    ordering: Vec<Member>,
}

/// The data structure underlying a webring. Implements the core webring functionality.
#[derive(Debug, Default)]
pub struct Webring {
    // https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    // This is a good case for std locks because we will never need to hold it across an await
    inner: RwLock<WebringData>,
    // This one we would like to hold across awaits
    homepage: tokio::sync::RwLock<Option<Arc<Homepage>>>,
    members_file_path: PathBuf,
    static_dir_path: PathBuf,
    file_watcher: OnceLock<RecommendedWatcher>,
    base_address: Uri,
    notifier: Option<Arc<DiscordNotifier>>,
}

impl Webring {
    /// Create a webring by parsing the file at the given path.
    ///
    /// The members' `check_successful` fields will be initialized to `true` until a check is
    /// performed using [`check_members()`][Self::check_members].
    pub async fn new(
        members_file: PathBuf,
        static_dir: PathBuf,
        base_address: Uri,
        notifier: Option<DiscordNotifier>,
    ) -> eyre::Result<Webring> {
        let webring_data = parse_file(&members_file).await?;

        Ok(Webring {
            inner: RwLock::new(webring_data),
            members_file_path: members_file,
            static_dir_path: static_dir,
            homepage: tokio::sync::RwLock::new(None),
            file_watcher: OnceLock::default(),
            base_address,
            notifier: notifier.map(Arc::new),
        })
    }

    /// Update the webring in-place by re-parsing the file given in the original `new` call, and
    /// invalidating the cache of the SSR'ed homepage. All new URLs are checked.
    ///
    /// Useful for updating the webring without restarting the server.
    pub async fn update_from_file_and_check(&self) -> eyre::Result<()> {
        let mut new_members = parse_file(&self.members_file_path).await?;
        let mut tasks = FuturesUnordered::new();

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
                        tasks.push(
                            new_members.ordering[*idx].check_and_store_and_optionally_notify(
                                &self.base_address,
                                self.notifier.as_ref().map(Arc::clone),
                            ),
                        );
                    }
                }
            }
        }

        // Wait for all tasks
        while tasks.next().await.is_some() {}

        *self.inner.write().unwrap() = new_members;
        *self.homepage.write().await = None;
        Ok(())
    }

    /// Query everyone's webpages and check them according to their respective check levels.
    pub async fn check_members(&self) {
        let mut tasks = self
            .inner
            .read()
            .unwrap()
            .ordering
            .iter()
            .map(|member| {
                member.check_and_store_and_optionally_notify(
                    &self.base_address,
                    self.notifier.as_ref().map(Arc::clone),
                )
            })
            .collect::<FuturesUnordered<_>>();

        // Wait for all tasks
        while tasks.next().await.is_some() {}

        *self.homepage.write().await = None;
    }

    fn member_idx_and_lock(
        &self,
        uri: &Uri,
    ) -> Result<(usize, RwLockReadGuard<'_, WebringData>), TraverseWebringError> {
        let inner = self.inner.read().unwrap();
        let authority = uri
            .authority()
            .ok_or_else(|| TraverseWebringError::NoAuthority(uri.to_owned()))?;
        Ok((
            *inner
                .members_table
                .get(authority)
                .ok_or_else(|| TraverseWebringError::AuthorityNotFound(authority.to_owned()))?,
            inner,
        ))
    }

    /// Get the next page in the webring from the given URI based on the authority part
    pub fn next_page(&self, uri: &Uri) -> Result<Arc<Uri>, TraverseWebringError> {
        let (mut idx, inner) = self.member_idx_and_lock(uri)?;

        // -1 to avoid jumping all the way around the ring to the same page
        for _ in 0..inner.ordering.len() - 1 {
            idx += 1;
            if idx == inner.ordering.len() {
                idx = 0;
            }

            if inner.ordering[idx].check_successful.load(Ordering::Relaxed) {
                return Ok(Arc::clone(&inner.ordering[idx].website));
            }
        }

        warn!("All webring members are broken???");

        Err(TraverseWebringError::AllMembersFailing)
    }

    /// Get the previous page in the webring from the given URI; based on the authority part
    pub fn prev_page(&self, uri: &Uri) -> Result<Arc<Uri>, TraverseWebringError> {
        let (mut idx, inner) = self.member_idx_and_lock(uri)?;

        // -1 to avoid jumping all the way around the ring to the same page
        for _ in 0..inner.ordering.len() - 1 {
            if idx == 0 {
                idx = inner.ordering.len();
            }
            idx -= 1;

            if inner.ordering[idx].check_successful.load(Ordering::Relaxed) {
                return Ok(Arc::clone(&inner.ordering[idx].website));
            }
        }

        warn!("All webring members are broken???");

        Err(TraverseWebringError::AllMembersFailing)
    }

    /// Get a random page in the webring.
    ///
    /// If the `origin` has a value, the returned page will be different from the one referred to
    /// by the value. This prevents a user who has a `/random` link on their site from being sent
    /// back to the site they came from.
    pub fn random_page(
        &self,
        maybe_origin: Option<&Uri>,
    ) -> Result<Arc<Uri>, TraverseWebringError> {
        let (maybe_idx, inner) =
            match maybe_origin.and_then(|origin| self.member_idx_and_lock(origin).ok()) {
                Some((idx, inner)) => (Some(idx), inner),
                None => (None, self.inner.read().unwrap()),
            };

        let mut range = (0..inner.ordering.len()).collect::<Vec<_>>();
        let mut range = &mut *range;

        let mut rng = rand::rng();

        while !range.is_empty() {
            let (chosen, rest) = range.partial_shuffle(&mut rng, 1);
            let chosen = chosen[0];

            if maybe_idx != Some(chosen)
                && inner.ordering[chosen]
                    .check_successful
                    .load(Ordering::Relaxed)
            {
                return Ok(Arc::clone(&inner.ordering[chosen].website));
            }

            range = rest;
        }

        warn!("All webring members are broken???");

        Err(TraverseWebringError::AllMembersFailing)
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
                        website: member_info.website.as_ref().into(),
                        check_successful: member_info.check_successful.load(Ordering::Relaxed),
                    })
                    .collect::<Vec<_>>()
            };

            let homepage = Arc::new(Homepage::new(&self.static_dir_path, &members).await?);

            *maybe_homepage = Some(Arc::clone(&homepage));

            Ok(homepage)
        }
    }

    /// Enable automatic reloading
    ///
    /// After calling this method, the `Webring`'s data will automatically be reloaded when the
    /// members file is changed. Similarly, the homepage will be invalidated when the template file
    /// is changed.
    ///
    /// This function must be called from a tokio runtime.
    pub fn enable_reloading(self: &Arc<Self>) -> eyre::Result<()> {
        let rt = tokio::runtime::Handle::current();
        let homepage_template_path = self.static_dir_path.join("index.html");
        let weak_webring = Arc::downgrade(self);
        let homepage_template_path_for_closure = homepage_template_path.clone();
        let mut watcher =
            notify::recommended_watcher(move |maybe_event: notify::Result<notify::Event>| {
                // We can upgrade because if the webring was dropped, this watcher would be
                // deregistered and thus we wouldn't be here.
                let webring = weak_webring.upgrade().unwrap();
                match maybe_event {
                    Ok(event) => {
                        debug!(
                            "Event observed on {}: {event:#?}",
                            webring.members_file_path.display()
                        );
                        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            return;
                        }

                        for path in event.paths {
                            if same_file::is_same_file(&path, &webring.members_file_path)
                                .unwrap_or_else(|err| {
                                    log::error!("Error comparing file paths: {err}");
                                    false
                                })
                            {
                                info!(
                                    "Detected change to {}. Reloading webring.",
                                    webring.members_file_path.display()
                                );
                                let webring_for_task = Arc::clone(&webring);
                                rt.spawn(async move {
                                    if let Err(err) =
                                        webring_for_task.update_from_file_and_check().await
                                    {
                                        error!("Failed to update webring: {err}");
                                    }
                                    info!("Webring reloaded");
                                });
                            }
                            if same_file::is_same_file(&path, &homepage_template_path_for_closure)
                                .unwrap_or_else(|err| {
                                    log::error!("Error comparing file paths: {err}");
                                    false
                                })
                            {
                                info!(
                                    "Detected change to {}. Invalidating homepage.",
                                    homepage_template_path_for_closure.display()
                                );
                                let webring_for_task = Arc::clone(&webring);
                                rt.spawn(async move {
                                    webring_for_task.homepage.write().await.take();
                                });
                            }
                        }
                    }
                    Err(err) => {
                        error!("Error watching file: {err}");
                    }
                }
            })?;
        watcher.watch(&self.members_file_path, RecursiveMode::NonRecursive)?;
        watcher.watch(&homepage_template_path, RecursiveMode::NonRecursive)?;
        let _ = self.file_watcher.set(watcher);
        Ok(())
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

        let check_level = CheckLevel::from_str(split[3])?;

        let uri = split[1].parse::<Uri>()?;

        let authority = match uri.authority() {
            Some(v) => v.to_owned(),
            None => return Err(eyre!("URLs must not be relative. Got: {uri}")),
        };

        let member = Member {
            name: split[0].to_owned(),
            website: Arc::from(uri),
            discord_id: match split[2] {
                "-" => None,
                id => Some(id.parse().wrap_err("Discord ID is not valid")?),
            },
            check_level,
            check_successful: Arc::new(AtomicBool::new(true)),
        };

        members
            .members_table
            .insert(authority, members.ordering.len());
        members.ordering.push(member);
    }

    Ok(members)
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TraverseWebringError {
    #[error("The given origin URI ({0}) doesn't have a host component")]
    NoAuthority(Uri),
    #[error("The given origin host ({0}) does not appear to be a member of the webring")]
    AuthorityNotFound(Authority),
    /// This may be returned even if the origin URI is passing because jumping to the same URI is undesirable
    #[error("All sites in the webring are currently down or failing our status checks")]
    AllMembersFailing,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use axum::http::{Uri, uri::Authority};
    use pretty_assertions::assert_eq;
    use tempfile::{NamedTempFile, TempDir};

    use crate::webring::{CheckLevel, Webring};

    use super::{Member, TraverseWebringError};

    impl PartialEq for Member {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
                && self.website == other.website
                && self.discord_id == other.discord_id
                && self.check_level == other.check_level
                && self.check_successful.load(Ordering::Relaxed)
                    == other.check_successful.load(Ordering::Relaxed)
        }
    }

    impl Webring {
        fn assert_prev(
            &self,
            addr: &'static str,
            prev: Result<&'static str, TraverseWebringError>,
        ) {
            assert_eq!(
                self.prev_page(&Uri::from_static(addr)),
                prev.map(|v| Arc::new(Uri::from_static(v)))
            );
        }

        fn assert_next(
            &self,
            addr: &'static str,
            next: Result<&'static str, TraverseWebringError>,
        ) {
            assert_eq!(
                self.next_page(&Uri::from_static(addr)),
                next.map(|v| Arc::new(Uri::from_static(v)))
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_webring() {
        let members_file = NamedTempFile::new().unwrap();
        tokio::fs::write(
            members_file.path(),
            "
henry — hrovnyak.gitlab.io — 123 — None
kian — kasad.com — 456 — NonE
cynthia — https://clementine.viridian.page — 789 — nONE
??? — ws://refuse-the-r.ing — - — none
",
        )
        .await
        .unwrap();
        let static_dir = TempDir::new().unwrap();

        let webring = Webring::new(
            members_file.path().to_owned(),
            static_dir.path().to_owned(),
            Uri::from_static("https://ring.purduehackers.com"),
            None,
        )
        .await
        .unwrap();

        {
            let inner = webring.inner.read().unwrap();

            let mut expected_table = HashMap::new();
            expected_table.insert(Authority::from_static("hrovnyak.gitlab.io"), 0);
            expected_table.insert(Authority::from_static("kasad.com"), 1);
            expected_table.insert(Authority::from_static("clementine.viridian.page"), 2);
            expected_table.insert(Authority::from_static("refuse-the-r.ing"), 3);

            assert_eq!(inner.members_table, expected_table);

            let expected_ordering = vec![
                Member {
                    name: "henry".to_owned(),
                    website: Arc::new(Uri::from_static("hrovnyak.gitlab.io")),
                    discord_id: Some("123".parse().unwrap()),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "kian".to_owned(),
                    website: Arc::new(Uri::from_static("kasad.com")),
                    discord_id: Some("456".parse().unwrap()),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "cynthia".to_owned(),
                    website: Arc::new(Uri::from_static("https://clementine.viridian.page")),
                    discord_id: Some("789".parse().unwrap()),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "???".to_owned(),
                    website: Arc::new(Uri::from_static("ws://refuse-the-r.ing")),
                    discord_id: None,
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
            ];
            assert_eq!(inner.ordering, expected_ordering);
        }

        webring.assert_next(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("kasad.com"),
        );

        webring.assert_prev(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("ws://refuse-the-r.ing"),
        );

        webring.assert_next("huh://refuse-the-r.ing", Ok("hrovnyak.gitlab.io"));
        webring.assert_prev(
            "https://kasad.com:3000",
            Err(TraverseWebringError::AuthorityNotFound(
                Authority::from_static("kasad.com:3000"),
            )),
        );
        webring.assert_next("https://kasad.com", Ok("https://clementine.viridian.page"));
        webring.assert_prev(
            "/relative/uri",
            Err(TraverseWebringError::NoAuthority(Uri::from_static(
                "/relative/uri",
            ))),
        );

        webring.inner.write().unwrap().ordering[0]
            .check_successful
            .store(false, Ordering::Relaxed);

        webring.assert_next(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("kasad.com"),
        );

        webring.assert_prev(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("ws://refuse-the-r.ing"),
        );

        webring.assert_next("refuse-the-r.ing", Ok("kasad.com"));
        webring.assert_prev("kasad.com", Ok("ws://refuse-the-r.ing"));

        let mut found_in_random = HashSet::new();

        // Test random with a specified origin matching a site
        let origin = Uri::from_static("ws://refuse-the-r.ing");
        for _ in 0..200 {
            found_in_random.insert((*webring.random_page(Some(&origin)).unwrap()).clone());
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://clementine.viridian.page"));
        assert_eq!(found_in_random, expected_random);

        // Test random without a specified origin
        found_in_random.clear();
        for _ in 0..200 {
            found_in_random.insert((*webring.random_page(None).unwrap()).clone());
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://clementine.viridian.page"));
        expected_random.insert(Uri::from_static("ws://refuse-the-r.ing"));
        assert_eq!(found_in_random, expected_random);

        // Test random with a specified origin not matching any site
        let origin = Uri::from_static("https://ring.purduehackers.com");
        found_in_random.clear();
        for _ in 0..200 {
            found_in_random.insert((*webring.random_page(Some(&origin)).unwrap()).clone());
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://clementine.viridian.page"));
        expected_random.insert(Uri::from_static("ws://refuse-the-r.ing"));
        assert_eq!(found_in_random, expected_random);

        tokio::fs::write(
            members_file.path(),
            "
cynthia — https://clementine.viridian.page — 789 — nONE
henry — hrovnyak.gitlab.io — 123 — None
??? — http://refuse-the-r.ing — 293847 — none
arhan — arhan.sh — 0 — none
kian — kasad.com — 456 — NonE
",
        )
        .await
        .unwrap();

        webring.update_from_file_and_check().await.unwrap();

        webring.assert_next("clementine.viridian.page", Ok("http://refuse-the-r.ing"));
        webring.assert_next("hrovnyak.gitlab.io", Ok("http://refuse-the-r.ing"));
        webring.assert_next("refuse-the-r.ing", Ok("arhan.sh"));
        webring.assert_next("arhan.sh", Ok("kasad.com"));
        webring.assert_next("kasad.com", Ok("https://clementine.viridian.page"));

        for i in 0..5 {
            webring.inner.write().unwrap().ordering[i]
                .check_successful
                .store(false, Ordering::Relaxed);
        }

        webring.assert_next("kasad.com", Err(TraverseWebringError::AllMembersFailing));
        webring.assert_prev(
            "clementine.viridian.page",
            Err(TraverseWebringError::AllMembersFailing),
        );
        assert_eq!(
            webring.random_page(None),
            Err(TraverseWebringError::AllMembersFailing)
        );
        webring.assert_prev(
            "clementine.viridian.page",
            Err(TraverseWebringError::AllMembersFailing),
        );

        webring.inner.write().unwrap().ordering[3]
            .check_successful
            .store(true, Ordering::Relaxed);

        for _ in 0..200 {
            assert_eq!(
                webring.random_page(None),
                Ok(Arc::new(Uri::from_static("arhan.sh")))
            );
        }

        webring.assert_prev("arhan.sh", Err(TraverseWebringError::AllMembersFailing));
        webring.assert_next("arhan.sh", Err(TraverseWebringError::AllMembersFailing));

        webring.assert_prev("refuse-the-r.ing", Ok("arhan.sh"));
        webring.assert_next("kasad.com", Ok("arhan.sh"));
    }

    /// Creates a members list file and a static directory containing an `index.html` file.
    ///
    /// Returns the [`TempDir`] containing all of the files, path of the members file, and the
    /// static directory, in that order.
    ///
    /// The [`TempDir`] isn't terribly useful, but once it is dropped, the files are cleaned up, so
    /// it must be returned.
    async fn create_files() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let static_dir_path = dir.path().join("static");
        tokio::fs::create_dir(&static_dir_path).await.unwrap();
        tokio::fs::File::create_new(static_dir_path.join("index.html"))
            .await
            .unwrap();
        let members_file_path = dir.path().join("members.txt");
        tokio::fs::File::create_new(&members_file_path)
            .await
            .unwrap();
        (dir, members_file_path, static_dir_path)
    }

    #[tokio::test]
    async fn test_reload_webring() {
        let (_dir, members_file, static_dir) = create_files().await;
        let file_contents = "\
cynthia — https://clementine.viridian.page — 789 — nONE";
        tokio::fs::write(&members_file, file_contents)
            .await
            .unwrap();
        let webring = Arc::new(
            Webring::new(
                members_file.clone(),
                static_dir,
                Uri::from_static("https://ring.purduehackers.com"),
                None,
            )
            .await
            .unwrap(),
        );
        webring.enable_reloading().unwrap();
        assert_eq!(webring.inner.read().unwrap().ordering.len(), 1);
        let new_file_contents = "\
cynthia — https://clementine.viridian.page — 789 — nONE
kian — kasad.com — 123 — none";
        tokio::fs::write(&members_file, new_file_contents)
            .await
            .unwrap();
        // Wait for a bit just in case the event takes some time to process
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(webring.inner.read().unwrap().ordering.len(), 2);
    }

    #[tokio::test]
    async fn test_reload_homepage() {
        let (_dir, members_file, static_dir) = create_files().await;
        let webring = Arc::new(Webring {
            static_dir_path: static_dir.clone(),
            members_file_path: members_file,
            ..Default::default()
        });
        webring.enable_reloading().unwrap();

        // Generate the homepage
        webring.homepage().await.unwrap();
        assert!(webring.homepage.read().await.is_some());

        // Change the file
        tokio::fs::write(static_dir.join("index.html"), "test")
            .await
            .unwrap();

        // Wait for a bit just in case the event takes some time to process
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Expect the homepage to be empty (invalidated)
        assert!(webring.homepage.read().await.is_none());
    }

    /// Test to ensure that the reloading logic doesn't create a reference cycle and the webring
    /// does actually get dropped.
    #[tokio::test]
    async fn test_reload_gets_dropped() {
        let (_dir, members_file, static_dir) = create_files().await;
        let webring = Arc::new(Webring {
            members_file_path: members_file,
            static_dir_path: static_dir,
            ..Default::default()
        });
        let weak_ptr = Arc::downgrade(&webring);
        webring.enable_reloading().unwrap();
        drop(webring);
        assert!(weak_ptr.upgrade().is_none());
    }
}
