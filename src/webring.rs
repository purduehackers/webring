use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use axum::http::{Uri, uri::Authority};
use eyre::eyre;
use futures::{StreamExt, stream::FuturesUnordered};
use rand::seq::SliceRandom;

use log::{error, warn};
use thiserror::Error;
use tokio::io::AsyncReadExt;

use crate::{
    checking::check,
    homepage::{Homepage, MemberForHomepage},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckLevel {
    ForLinks,
    JustOnline,
    None,
}

#[derive(Clone, Debug)]
struct Member {
    name: String,
    website: Arc<Uri>,
    #[allow(dead_code)] // FIXME: Remove once Discord integration is implemented
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
            if let Some(v) = check(&website, check_level).await {
                successful.store(v, Ordering::Relaxed);
            }

            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
struct WebringData {
    members_table: HashMap<Authority, usize>,
    ordering: Vec<Member>,
    source_file_last_modified: SystemTime,
}

/// The data structure underlying a webring. Implements the core webring functionality.
#[derive(Debug)]
pub struct Webring {
    // https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    // This is a good case for std locks because we will never need to hold it across an await
    inner: RwLock<WebringData>,
    // This one we would like to hold across awaits
    homepage: tokio::sync::RwLock<Option<Arc<Homepage>>>,
    members_file_path: PathBuf,
    static_dir_path: PathBuf,
}

impl Webring {
    /// Create a webring by parsing the file at the given path.
    pub async fn new(members_file: PathBuf, static_dir: PathBuf) -> eyre::Result<Webring> {
        let webring_data = parse_file(&members_file).await;

        let webring = Webring {
            inner: RwLock::new(webring_data?),
            members_file_path: members_file,
            static_dir_path: static_dir,
            homepage: tokio::sync::RwLock::new(None),
        };

        webring.check_members().await?;

        Ok(webring)
    }

    /// Re-load the [`Webring`] from its source file if the source file has been modified since the
    /// last time we loaded it.
    ///
    /// Any errors returned by [`update_from_file()`] as part of this operation are logged and then
    /// ignored.
    pub async fn try_update(&self) {
        let last_modified = self.inner.read().unwrap().source_file_last_modified;
        let source_file = &self.members_file_path;
        let metadata = match tokio::fs::metadata(source_file).await {
            Ok(m) => m,
            Err(err) => {
                error!("Error getting member file metadata: {err}");
                return;
            }
        };
        if metadata
            .modified()
            .expect("System does not support modification timestamps")
            > last_modified
        {
            if let Err(err) = self.update_from_file().await {
                error!("Members list modified but failed to reload: {err}");
            }
        }
    }

    /// Update the webring in-place by re-parsing the file given in the original `new` call, and invalidating the cache of the SSR'ed homepage.
    ///
    /// Useful for updating the webring without restarting the server.
    pub async fn update_from_file(&self) -> eyre::Result<()> {
        let mut new_members = parse_file(&self.members_file_path).await?;
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

        *self.inner.write().unwrap() = new_members;

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
    pub async fn next_page(&self, uri: &Uri) -> Result<Arc<Uri>, TraverseWebringError> {
        self.try_update().await;
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
    pub async fn prev_page(&self, uri: &Uri) -> Result<Arc<Uri>, TraverseWebringError> {
        self.try_update().await;
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
    pub async fn random_page(
        &self,
        maybe_origin: Option<&Uri>,
    ) -> Result<Arc<Uri>, TraverseWebringError> {
        self.try_update().await;
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
    let mut file = tokio::fs::File::open(&path).await?;
    let mut file_contents = String::new();
    file.read_to_string(&mut file_contents).await?;

    let mut members = WebringData {
        members_table: HashMap::new(),
        ordering: Vec::new(),
        source_file_last_modified: file
            .metadata()
            .await?
            .modified()
            .expect("System does not support file modification timestamp"),
    };

    for line in file_contents.lines().filter(|line| !line.is_empty()) {
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
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use axum::http::{Uri, uri::Authority};
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
        async fn assert_prev(
            &self,
            addr: &'static str,
            prev: Result<&'static str, TraverseWebringError>,
        ) {
            assert_eq!(
                self.prev_page(&Uri::from_static(addr)).await,
                prev.map(|v| Arc::new(Uri::from_static(v)))
            );
        }

        async fn assert_next(
            &self,
            addr: &'static str,
            next: Result<&'static str, TraverseWebringError>,
        ) {
            assert_eq!(
                self.next_page(&Uri::from_static(addr)).await,
                next.map(|v| Arc::new(Uri::from_static(v)))
            );
        }
    }

    const MEMBERS_FILE_TEXT: &str = "
henry — hrovnyak.gitlab.io — 123 — None
kian — kasad.com — 456 — NonE
cynthia — https://clementine.viridian.page — 789 — nONE
??? — ws://refuse-the-r.ing — bruh — none
";

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_webring() {
        let members_file = NamedTempFile::new().unwrap();
        tokio::fs::write(members_file.path(), MEMBERS_FILE_TEXT)
            .await
            .unwrap();
        let static_dir = TempDir::new().unwrap();

        let webring = Webring::new(members_file.path().to_owned(), static_dir.path().to_owned())
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
                    discord_id: "123".to_owned(),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "kian".to_owned(),
                    website: Arc::new(Uri::from_static("kasad.com")),
                    discord_id: "456".to_owned(),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "cynthia".to_owned(),
                    website: Arc::new(Uri::from_static("https://clementine.viridian.page")),
                    discord_id: "789".to_owned(),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
                Member {
                    name: "???".to_owned(),
                    website: Arc::new(Uri::from_static("ws://refuse-the-r.ing")),
                    discord_id: "bruh".to_owned(),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                },
            ];
            assert_eq!(inner.ordering, expected_ordering);
        }

        webring
            .assert_next(
                "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
                Ok("kasad.com"),
            )
            .await;

        webring
            .assert_prev(
                "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
                Ok("ws://refuse-the-r.ing"),
            )
            .await;

        webring
            .assert_next("huh://refuse-the-r.ing", Ok("hrovnyak.gitlab.io"))
            .await;
        webring
            .assert_prev(
                "https://kasad.com:3000",
                Err(TraverseWebringError::AuthorityNotFound(
                    Authority::from_static("kasad.com:3000"),
                )),
            )
            .await;
        webring
            .assert_next("https://kasad.com", Ok("https://clementine.viridian.page"))
            .await;
        webring
            .assert_prev(
                "/relative/uri",
                Err(TraverseWebringError::NoAuthority(Uri::from_static(
                    "/relative/uri",
                ))),
            )
            .await;

        webring.inner.write().unwrap().ordering[0]
            .check_successful
            .store(false, Ordering::Relaxed);

        webring
            .assert_next(
                "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
                Ok("kasad.com"),
            )
            .await;

        webring
            .assert_prev(
                "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
                Ok("ws://refuse-the-r.ing"),
            )
            .await;

        webring
            .assert_next("refuse-the-r.ing", Ok("kasad.com"))
            .await;
        webring
            .assert_prev("kasad.com", Ok("ws://refuse-the-r.ing"))
            .await;

        let mut found_in_random = HashSet::new();

        // Test random with a specified origin matching a site
        let origin = Uri::from_static("ws://refuse-the-r.ing");
        for _ in 0..200 {
            found_in_random.insert((*webring.random_page(Some(&origin)).await.unwrap()).clone());
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://clementine.viridian.page"));
        assert_eq!(found_in_random, expected_random);

        // Test random without a specified origin
        found_in_random.clear();
        for _ in 0..200 {
            found_in_random.insert((*webring.random_page(None).await.unwrap()).clone());
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
            found_in_random.insert((*webring.random_page(Some(&origin)).await.unwrap()).clone());
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
??? — http://refuse-the-r.ing — bruh — none
arhan — arhan.sh — qter — none
kian — kasad.com — 456 — NonE
",
        )
        .await
        .unwrap();

        webring.update_from_file().await.unwrap();

        webring
            .assert_next("clementine.viridian.page", Ok("http://refuse-the-r.ing"))
            .await;
        webring
            .assert_next("hrovnyak.gitlab.io", Ok("http://refuse-the-r.ing"))
            .await;
        webring
            .assert_next("refuse-the-r.ing", Ok("arhan.sh"))
            .await;
        webring.assert_next("arhan.sh", Ok("kasad.com")).await;
        webring
            .assert_next("kasad.com", Ok("https://clementine.viridian.page"))
            .await;

        for i in 0..5 {
            webring.inner.write().unwrap().ordering[i]
                .check_successful
                .store(false, Ordering::Relaxed);
        }

        webring
            .assert_next("kasad.com", Err(TraverseWebringError::AllMembersFailing))
            .await;
        webring
            .assert_prev(
                "clementine.viridian.page",
                Err(TraverseWebringError::AllMembersFailing),
            )
            .await;
        assert_eq!(
            webring.random_page(None).await,
            Err(TraverseWebringError::AllMembersFailing)
        );
        webring
            .assert_prev(
                "clementine.viridian.page",
                Err(TraverseWebringError::AllMembersFailing),
            )
            .await;

        webring.inner.write().unwrap().ordering[3]
            .check_successful
            .store(true, Ordering::Relaxed);

        for _ in 0..200 {
            assert_eq!(
                webring.random_page(None).await,
                Ok(Arc::new(Uri::from_static("arhan.sh")))
            );
        }

        webring
            .assert_prev("arhan.sh", Err(TraverseWebringError::AllMembersFailing))
            .await;
        webring
            .assert_next("arhan.sh", Err(TraverseWebringError::AllMembersFailing))
            .await;

        webring
            .assert_prev("refuse-the-r.ing", Ok("arhan.sh"))
            .await;
        webring.assert_next("kasad.com", Ok("arhan.sh")).await;
    }

    #[tokio::test]
    async fn test_reload() {
        // Create a members file containing all but the last line in the original MEMBERS_FILE_TEXT
        let members_lines: Vec<&str> = MEMBERS_FILE_TEXT.lines().collect();
        let truncated_members_text = {
            let mut s = members_lines[..members_lines.len() - 1].join("\n");
            s.push('\n');
            s
        };
        let members_file = NamedTempFile::new().unwrap();
        tokio::fs::write(members_file.path(), truncated_members_text)
            .await
            .unwrap();

        // Create a webring from this file
        let static_dir = TempDir::new().unwrap();
        let webring = Webring::new(members_file.path().to_owned(), static_dir.path().to_owned())
            .await
            .unwrap();

        // Check initial expected members
        let mut expected_table = HashMap::new();
        expected_table.insert(Authority::from_static("hrovnyak.gitlab.io"), 0);
        expected_table.insert(Authority::from_static("kasad.com"), 1);
        expected_table.insert(Authority::from_static("clementine.viridian.page"), 2);
        assert_eq!(webring.inner.read().unwrap().members_table, expected_table);

        // Check initial expected members
        let mut expected_ordering = vec![
            Member {
                name: "henry".to_owned(),
                website: Arc::new(Uri::from_static("hrovnyak.gitlab.io")),
                discord_id: "123".to_owned(),
                check_level: CheckLevel::None,
                check_successful: Arc::new(AtomicBool::new(true)),
            },
            Member {
                name: "kian".to_owned(),
                website: Arc::new(Uri::from_static("kasad.com")),
                discord_id: "456".to_owned(),
                check_level: CheckLevel::None,
                check_successful: Arc::new(AtomicBool::new(true)),
            },
            Member {
                name: "cynthia".to_owned(),
                website: Arc::new(Uri::from_static("https://clementine.viridian.page")),
                discord_id: "789".to_owned(),
                check_level: CheckLevel::None,
                check_successful: Arc::new(AtomicBool::new(true)),
            },
        ];
        assert_eq!(webring.inner.read().unwrap().ordering, expected_ordering);

        // Append last member to webring file
        tokio::fs::write(members_file.path(), MEMBERS_FILE_TEXT)
            .await
            .unwrap();

        // Attempt to update
        webring.try_update().await;

        // Add last user to expectations
        expected_table.insert(Authority::from_static("refuse-the-r.ing"), 3);
        expected_ordering.push(Member {
            name: "???".to_owned(),
            website: Arc::new(Uri::from_static("ws://refuse-the-r.ing")),
            discord_id: "bruh".to_owned(),
            check_level: CheckLevel::None,
            check_successful: Arc::new(AtomicBool::new(true)),
        });

        // Check that new data matches
        assert_eq!(webring.inner.read().unwrap().members_table, expected_table);
        assert_eq!(webring.inner.read().unwrap().members_table, expected_table);
    }
}
