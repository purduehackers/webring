//! Ring behavior and data structures

use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::http::{Uri, uri::Authority};
use chrono::TimeDelta;
use futures::{StreamExt, future::join, stream::FuturesUnordered};
use indexmap::IndexMap;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use rand::seq::SliceRandom;
use tokio::{
    sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock},
    task::JoinHandle,
    time::Instant,
};

use sarlacc::Intern;
use serde::Deserialize;
use thiserror::Error;
use tracing::{Instrument, debug, error, field::display, info, info_span, instrument, warn};

use crate::{
    checking::check,
    config::{Config, MemberSpec},
    discord::{DiscordNotifier, NOTIFICATION_DEBOUNCE_PERIOD, Snowflake},
    homepage::{Homepage, MemberForHomepage},
    stats::{Stats, UNKNOWN_ORIGIN},
};

/// Represents the level of checking to perform on a member's website.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckLevel {
    /// Don't perform any checks; site is always considered available.
    #[serde(alias = "off")]
    None,
    /// Perform a basic check to see if the site is online, i.e. returns an HTTP 2xx response.
    #[serde(rename = "online", alias = "up")]
    JustOnline,
    /// Check that the site is online and that its webring links are present and valid.
    #[serde(rename = "links", alias = "full")]
    ForLinks,
}

impl Default for CheckLevel {
    /// Default check level for a member if none is specified in the configuration
    fn default() -> Self {
        CheckLevel::ForLinks
    }
}

/// Ring member
#[derive(Clone, Debug)]
struct Member {
    /// Name of the member, primarily for display on the homepage
    name: String,
    /// URI of the member's site
    website: Intern<Uri>,
    /// Authority (a.k.a. host or domain) of the member's site
    authority: Intern<Authority>,
    /// Discord ID of the member, if they opt in to [Discord integration][crate::discord].
    discord_id: Option<Snowflake>,
    /// Level of checking to perform on the member's site
    check_level: CheckLevel,
    /// Whether the last check was successful
    check_successful: Arc<AtomicBool>,
    /// The last time the member was notified of a failure.
    last_notified: Arc<AsyncMutex<Option<Instant>>>,
}

impl From<(&str, &MemberSpec)> for Member {
    fn from((name, spec): (&str, &MemberSpec)) -> Self {
        Self {
            name: name.to_owned(),
            website: spec.uri(),
            authority: Intern::from_ref(spec.uri().authority().unwrap()),
            discord_id: spec.discord_id,
            check_level: spec.check_level,
            check_successful: Arc::new(AtomicBool::new(true)),
            last_notified: Arc::new(AsyncMutex::new(None)),
        }
    }
}

impl Member {
    /// Checks the member's site, and stores the result. If the check fails and the member has
    /// opted in to notifications, also notifies them of the failure.
    ///
    /// Returns a future that resolves to the handle of the spawned task that sends the Discord
    /// notification, if any. (Mainly intended for testing purposes.)
    #[instrument(name = "webring.check_member", skip_all, fields(site))]
    fn check_and_store_and_optionally_notify(
        &self,
        base_address: Intern<Uri>,
        notifier: Option<Arc<DiscordNotifier>>,
    ) -> impl Future<Output = Option<JoinHandle<()>>> + Send + 'static {
        tracing::Span::current().record("site", display(&self.website));

        let website = self.website;
        let check_level = self.check_level;
        let successful = Arc::clone(&self.check_successful);

        let discord_id_for_block = self.discord_id;
        let base_address_for_block = base_address;
        let last_notified_for_block = Arc::clone(&self.last_notified);
        async move {
            let Ok(check_result) = check(&website, check_level, base_address_for_block).await
            else {
                return None;
            };

            debug!(site = %website, ?check_result, "got check result for member site");
            if let Some(failure) = check_result {
                let prev_was_successful = successful.swap(false, Ordering::Relaxed);
                if let (Some(notifier), Some(user_id)) = (notifier, discord_id_for_block) {
                    // Notifications are enabled. Send notification asynchronously.
                    Some(tokio::spawn(async move {
                        // If the last check was successful or the last notification was sent more than
                        // a day ago, notify the user.
                        let mut last_notified = last_notified_for_block.lock().await;
                        if prev_was_successful
                            || last_notified.is_none_or(|last| {
                                Instant::now().duration_since(last) > NOTIFICATION_DEBOUNCE_PERIOD
                            })
                        {
                            let message = format!("<@{}> {}", user_id, failure.to_message());
                            if notifier.send_message(Some(user_id), &message).await.is_ok() {
                                *last_notified = Some(Instant::now());
                            }
                        }
                    }))
                } else {
                    None
                }
            } else {
                successful.store(true, Ordering::Relaxed);
                None
            }
        }
    }
}

/// Map of members in the webring, indexed by their authority (host).
type MemberMap = IndexMap<Intern<Authority>, Member>;

/// Constructs a [`MemberMap`] from the `members` table of a [`Config`] object.
fn member_map_from_config_table(config_table: &IndexMap<String, MemberSpec>) -> MemberMap {
    config_table
        .into_iter()
        .map(|(name, spec)| {
            let authority = Intern::new(spec.uri().authority().unwrap().clone());
            let member = Member::from((name.as_str(), spec));
            (authority, member)
        })
        .collect()
}

/// The data structure underlying a webring. Implements the core webring functionality.
#[derive(Debug)]
pub struct Webring {
    /// Map of members in the webring, indexed by their authority (host).
    // https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    // This is a good case for std locks because we will never need to hold it across an await
    members: RwLock<MemberMap>,
    /// Cached rendered homepage
    // This one we would like to hold across awaits
    homepage: AsyncRwLock<Option<Arc<Homepage>>>,
    /// Directory where static content is to be served from.
    static_dir_path: PathBuf,
    /// File watcher for reloading the webring or homepage when the config file or homepage
    /// template changes
    file_watcher: OnceLock<RecommendedWatcher>,
    /// Base address of the webring, used to know what fully-formed webring links should look like.
    base_address: Intern<Uri>,
    /// Base authority of the webring
    base_authority: Intern<Authority>,
    /// Discord notifier for notifying members of issues with their sites
    notifier: Option<Arc<DiscordNotifier>>,
    /// Statistics collected about the ring
    stats: Arc<Stats>,
    /// Current configuration of the webring, used for detecting changes when reloading
    config: Arc<AsyncRwLock<Option<Config>>>,
}

impl Webring {
    /// Create a webring by parsing the file at the given path.
    ///
    /// The members' `check_successful` fields will be initialized to `true` until a check is
    /// performed using [`check_members()`][Self::check_members].
    pub fn new(config: &Config) -> Webring {
        Webring {
            members: RwLock::new(member_map_from_config_table(&config.members)),
            static_dir_path: config.webring.static_dir.clone(),
            homepage: AsyncRwLock::new(None),
            stats: Arc::new(Stats::new()),
            file_watcher: OnceLock::default(),
            base_address: config.webring.base_url(),
            notifier: config
                .discord
                .as_ref()
                .map(|dt| &dt.webhook_url)
                .map(DiscordNotifier::new)
                .map(Arc::new),
            base_authority: Intern::from_ref(config.webring.base_url().authority().unwrap()),
            config: Arc::new(AsyncRwLock::new(Some(config.clone()))),
        }
    }

    /// Update the webring's members in-place and invalidate the cache of the SSR'ed homepage.
    /// All new URLs are checked.
    ///
    /// Useful for updating the webring without restarting the server.
    pub async fn update_members_and_check(&self, member_specs: &IndexMap<String, MemberSpec>) {
        let mut new_members = member_map_from_config_table(member_specs);
        let mut tasks = FuturesUnordered::new();

        {
            let old_members = self.members.read().unwrap();

            for (name, new_member) in &mut new_members {
                match old_members.get(name) {
                    Some(old_member) => {
                        new_member.check_successful = Arc::clone(&old_member.check_successful);
                        if old_member.check_level != new_member.check_level {
                            tasks.push(new_member.check_and_store_and_optionally_notify(
                                self.base_address,
                                self.notifier.as_ref().map(Arc::clone),
                            ));
                        }
                    }
                    None => {
                        tasks.push(new_member.check_and_store_and_optionally_notify(
                            self.base_address,
                            self.notifier.as_ref().map(Arc::clone),
                        ));
                    }
                }
            }
        }

        // Wait for all tasks
        while tasks.next().await.is_some() {}

        *self.members.write().unwrap() = new_members;
        *self.homepage.write().await = None;
    }

    /// Performs checks on all members of the webring, updating their `check_successful` fields and
    /// notifying them if configured.
    #[instrument(name = "webring.check_members", skip(self))]
    pub async fn check_members(&self) {
        let mut tasks = self
            .members
            .read()
            .unwrap()
            .iter()
            .map(|(_, member)| {
                member.check_and_store_and_optionally_notify(
                    self.base_address,
                    self.notifier.as_ref().map(Arc::clone),
                )
            })
            .collect::<FuturesUnordered<_>>();

        // Wait for all tasks
        while tasks.next().await.is_some() {}

        *self.homepage.write().await = None;
    }

    /// Get the authority part of the given URI, returning an error if it doesn't have one or if it
    /// doesn't exist in the intern table.
    fn get_authority(uri: &Uri) -> Result<Intern<Authority>, TraverseWebringError> {
        let authority = uri
            .authority()
            .ok_or_else(|| TraverseWebringError::NoAuthority(uri.to_owned()))?;
        Intern::get_ref(authority)
            .ok_or_else(|| TraverseWebringError::AuthorityNotFound(authority.to_owned()))
    }

    /// Gets the index of the member in the webring and a read lock on the member map.
    fn member_idx_and_lock(
        &self,
        uri: &Uri,
    ) -> Result<(usize, Intern<Authority>, RwLockReadGuard<'_, MemberMap>), TraverseWebringError>
    {
        let interned = Self::get_authority(uri)?;
        let inner = self.members.read().unwrap();
        Ok((
            inner.get_index_of(&interned).ok_or_else(|| {
                TraverseWebringError::AuthorityNotFound(uri.authority().unwrap().to_owned())
            })?,
            interned,
            inner,
        ))
    }

    /// Gets the next page in the webring from the given URI based on the authority part
    pub fn next_page(&self, uri: &Uri, ip: IpAddr) -> Result<Intern<Uri>, TraverseWebringError> {
        let (mut idx, authority, inner) = self.member_idx_and_lock(uri)?;

        // -1 to avoid jumping all the way around the ring to the same page
        for _ in 0..inner.len() - 1 {
            idx += 1;
            if idx == inner.len() {
                idx = 0;
            }

            if inner[idx].check_successful.load(Ordering::Relaxed) {
                self.stats.redirected(ip, authority, inner[idx].authority);
                return Ok(inner[idx].website);
            }
        }

        warn!("All webring members are broken???");

        Err(TraverseWebringError::AllMembersFailing)
    }

    /// Get the previous page in the webring from the given URI; based on the authority part
    pub fn prev_page(&self, uri: &Uri, ip: IpAddr) -> Result<Intern<Uri>, TraverseWebringError> {
        let (mut idx, authority, inner) = self.member_idx_and_lock(uri)?;

        // -1 to avoid jumping all the way around the ring to the same page
        for _ in 0..inner.len() - 1 {
            if idx == 0 {
                idx = inner.len();
            }
            idx -= 1;

            if inner[idx].check_successful.load(Ordering::Relaxed) {
                self.stats.redirected(ip, authority, inner[idx].authority);
                return Ok(inner[idx].website);
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
    ///
    /// If `origin` does not have a value, the request will be logged as coming from an unknown source.
    pub fn random_page(
        &self,
        maybe_origin: Option<&Uri>,
        ip: IpAddr,
    ) -> Result<Intern<Uri>, TraverseWebringError> {
        let (maybe_idx, maybe_authority, inner) =
            match maybe_origin.and_then(|origin| self.member_idx_and_lock(origin).ok()) {
                Some((idx, authority, inner)) => (Some(idx), Some(authority), inner),
                None => (None, None, self.members.read().unwrap()),
            };

        let mut range = (0..inner.len()).collect::<Vec<_>>();
        let mut range = &mut *range;

        let mut rng = rand::rng();

        while !range.is_empty() {
            let (chosen, rest) = range.partial_shuffle(&mut rng, 1);
            let chosen = chosen[0];

            if maybe_idx != Some(chosen) && inner[chosen].check_successful.load(Ordering::Relaxed) {
                self.stats.redirected(
                    ip,
                    maybe_authority.unwrap_or_else(|| *UNKNOWN_ORIGIN),
                    inner[chosen].authority,
                );

                return Ok(inner[chosen].website);
            }

            range = rest;
        }

        warn!("All webring members are broken???");

        Err(TraverseWebringError::AllMembersFailing)
    }

    /// Track a click to the webring homepage. If there is no authority for the origin or the authority isn't found, this will say that the source is "unknown".
    pub fn track_to_homepage_click(&self, maybe_member_page: Option<&Uri>, ip: IpAddr) {
        let interned = maybe_member_page
            .and_then(|member_page| Self::get_authority(member_page).ok())
            .unwrap_or_else(|| *UNKNOWN_ORIGIN);
        self.stats.redirected(ip, interned, self.base_authority);
    }

    /// Track a click from the webring homepage to a ring member. This returns a result because redirecting someone to an invalid URI is a problem on our side.
    pub fn track_from_homepage_click(
        &self,
        member_page: &Uri,
        ip: IpAddr,
    ) -> Result<Intern<Uri>, TraverseWebringError> {
        let (idx, authority, inner) = self.member_idx_and_lock(member_page)?;

        self.stats.redirected(ip, self.base_authority, authority);

        Ok(inner[idx].website)
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
                let inner = self.members.read().unwrap();

                inner
                    .iter()
                    .map(|(_, member_info)| MemberForHomepage {
                        name: member_info.name.clone(),
                        website: member_info.website.as_ref().into(),
                        check_successful: member_info.check_successful.load(Ordering::Relaxed),
                    })
                    .collect::<Vec<_>>()
            };

            let homepage =
                Arc::new(Homepage::new(&self.static_dir_path, self.base_address, &members).await?);

            *maybe_homepage = Some(Arc::clone(&homepage));

            Ok(homepage)
        }
    }

    /// Enable automatic reloading
    ///
    /// After calling this method, the [`Webring`]'s data will automatically be reloaded when the
    /// given configuration file is changed. Similarly, the homepage will be invalidated when the
    /// template file is changed.
    ///
    /// Each webring can have at most one file watcher performing reloading. If this method is
    /// called multiple times, only the first call has an effect.
    ///
    /// This function must be called from a tokio runtime.
    #[instrument(skip(self))]
    pub fn enable_reloading(self: &Arc<Self>, config_file: &Path) -> eyre::Result<()> {
        /// Represents a detected live reload event.
        #[derive(Debug, Copy, Clone, PartialEq, Eq)]
        enum LiveReloadEvent {
            /// Reload the configuration file
            Config,
            /// Reload the homepage
            Homepage,
        }

        /// Check if two paths refer to the same file
        fn files_match(a: &Path, b: &Path) -> bool {
            same_file::is_same_file(a, b).unwrap_or_else(|err| {
                error!(%err, "Error comparing file paths");
                false
            })
        }

        let rt = tokio::runtime::Handle::current();
        let homepage_template = self.static_dir_path.join("index.html");
        let homepage_template_for_closure = homepage_template.clone();
        let config_file_for_closure = config_file.to_owned();
        let weak_webring = Arc::downgrade(self);
        let mut watcher = notify::recommended_watcher(
            move |maybe_event: Result<notify::Event, notify::Error>| {
                let _enter = info_span!("handle file event").entered();
                match maybe_event {
                    Ok(event) => {
                        for path in &event.paths {
                            debug!(file = %path.display(), ?event.kind, "Event observed");
                        }
                        // We only care about events that update the file
                        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            return;
                        }

                        // Keep track of which kind of reload to perform, so we only do one per
                        // group of paths changed.
                        let mut reload_kind = None;

                        for path in &event.paths {
                            // Handle homepage template update
                            if files_match(path, &homepage_template_for_closure) {
                                if reload_kind.is_none() {
                                    reload_kind = Some(LiveReloadEvent::Homepage);
                                }
                            }
                            // Handle config file update
                            else if files_match(path, &config_file_for_closure) {
                                reload_kind = Some(LiveReloadEvent::Config);
                            }
                            // A file we weren't watching was updated?
                            else {
                                warn!(file = %path.display(), "Unexpected file updated");
                            }
                        }

                        if let Some(kind) = reload_kind {
                            let config_file_path_for_task = config_file_for_closure.clone();
                            // This must succeed, because if the webring was dropped, so would be
                            // the thread running the event handler.
                            let webring = weak_webring.upgrade().unwrap();
                            rt.spawn(async move {
                                // This must succeed, because the webring
                                match kind {
                                    LiveReloadEvent::Config => {
                                        info!("Configuration file updated; reloading members");
                                        // Load new config
                                        match Config::parse_from_file(&config_file_path_for_task)
                                            .await
                                        {
                                            Ok(new_config) => {
                                                // Do these tasks concurrently since they don't
                                                // depend on each other.
                                                join(
                                                    async {
                                                        // If fields other than the members were changed,
                                                        // warn about it.
                                                        let old_config = webring.config.read().await;
                                                        if old_config.as_ref().is_some_and(|old| Config::diff_settings(old, &new_config)) {
                                                            warn!("Some non-member settings were changed. These will not be applied until the webring is restarted.");
                                                        }
                                                    },
                                                    async {
                                                        // Update webring from new member list
                                                        webring
                                                            .update_members_and_check(&new_config.members)
                                                            .await;
                                                    }
                                                ).await;
                                                *webring.config.write().await = Some(new_config);
                                            }
                                            Err(err) => {
                                                error!(
                                                    file = %config_file_path_for_task.display(),
                                                    %err,
                                                    "Failed to load configuration from file",
                                                );
                                            }
                                        }
                                    }
                                    LiveReloadEvent::Homepage => {
                                        info!(
                                            "Homepage template updated; invalidating cached homepage"
                                        );
                                        webring.invalidate_homepage().await;
                                    }
                                }
                            }.instrument(info_span!("config reload", file = ?config_file_for_closure)));
                        }
                    }
                    Err(err) => {
                        for path in &err.paths {
                            error!(file = %path.display(), %err, "Error watching file");
                        }
                    }
                }
            },
        )?;

        // Register the files to be watched
        watcher.watch(&homepage_template, RecursiveMode::NonRecursive)?;
        watcher.watch(config_file, RecursiveMode::NonRecursive)?;

        // We don't care if this succeeds or not. As long as there's one watcher, we're fine. If
        // this operation fails, the new watcher will just be dropped and cleaned up.
        let _ = self.file_watcher.set(watcher);

        Ok(())
    }

    /// Create a task that prunes IP addresses from the table at the given interval. Note that IPs will be pruned if they haven't been seen since `IP_TRACKING_TTL`.
    pub fn enable_ip_pruning(&self, interval: TimeDelta) {
        let stats_for_task = Arc::downgrade(&self.stats);
        let std_duration = interval.to_std().unwrap();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std_duration).await;

                // If the stats were freed that means the webring was freed and we can go die
                if let Some(stats) = stats_for_task.upgrade() {
                    stats.prune_seen_ips();
                } else {
                    return;
                }
            }
        });
    }

    /// Invalidate the homepage, forcing the next request for it to re-generate it from the
    /// template.
    pub async fn invalidate_homepage(&self) {
        *self.homepage.write().await = None;
    }

    #[cfg(test)]
    pub fn assert_stat_entry(&self, entry: (chrono::NaiveDate, &str, &str, &str), count: u64) {
        self.stats.assert_stat_entry(entry, count);
    }
}

/// Errors that can occur when traversing the webring.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TraverseWebringError {
    /// The given origin URI does not have a host component, i.e. it is not an absolute URI.
    #[error("The given origin URI ({0}) doesn't have a host component")]
    NoAuthority(Uri),
    /// An origin was given but it doesn't match any member of the webring.
    #[error("The given origin host ({0}) does not appear to be a member of the webring")]
    AuthorityNotFound(Authority),
    /// No members are considered available (i.e. passing checks), so there's no one to route to.
    /// This may be returned even if the origin site is available because jumping to the same site is undesirable.
    #[error("All sites in the webring are currently down or failing our status checks")]
    AllMembersFailing,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        fs::{File, OpenOptions},
        io::Write as _,
        path::PathBuf,
        sync::{
            Arc, OnceLock, RwLock,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use axum::{
        Router,
        http::{Uri, uri::Authority},
        routing::get,
    };
    use chrono::Utc;
    use httpmock::prelude::*;
    use indexmap::IndexMap;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use sarlacc::Intern;
    use tempfile::{NamedTempFile, TempDir};
    use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};

    use crate::{
        config::{Config, MemberSpec},
        discord::{DiscordNotifier, NOTIFICATION_DEBOUNCE_PERIOD, Snowflake},
        stats::{TIMEZONE, UNKNOWN_ORIGIN},
        webring::{CheckLevel, Webring},
    };

    use super::{Member, TraverseWebringError};

    // `Authority` has no `Default` impl so we have to do it manually
    impl Default for Webring {
        fn default() -> Self {
            Webring {
                members: RwLock::default(),
                homepage: AsyncRwLock::default(),
                static_dir_path: PathBuf::default(),
                file_watcher: OnceLock::new(),
                base_address: Intern::default(),
                base_authority: Intern::new("ring.purduehackers.com".parse().unwrap()),
                notifier: None,
                stats: Arc::default(),
                config: Arc::new(AsyncRwLock::new(None)),
            }
        }
    }

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
                self.prev_page(&Uri::from_static(addr), "0.0.0.0".parse().unwrap()),
                prev.map(|v| Intern::new(Uri::from_static(v)))
            );
        }

        fn assert_next(
            &self,
            addr: &'static str,
            next: Result<&'static str, TraverseWebringError>,
        ) {
            assert_eq!(
                self.next_page(&Uri::from_static(addr), "0.0.0.0".parse().unwrap()),
                next.map(|v| Intern::new(Uri::from_static(v)))
            );
        }
    }

    fn make_config() -> Config {
        // Start a web server so we can make sure that it does checks right
        let server_addr = ("127.0.0.1", 32751);
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();
            let router: Router<()> = Router::new().route("/up", get(async || "Hi there!"));
            axum::serve(listener, router).await.unwrap();
        });

        let static_dir = TempDir::new().unwrap();
        toml::from_str(&format!(
            indoc! { r#"
                [webring]
                static-dir = "{}"
                base-url = "https://ring.purduehackers.com"

                [network]
                listen-addr = "0.0.0.0:3000"

                [members]
                henry = {{ url = "hrovnyak.gitlab.io", discord-id = 123, check-level = "none" }}
                kian = {{ url = "kasad.com", discord-id = 456, check-level = "none" }}
                cynthia = {{ url = "https://localhost:32751", discord-id = 789, check-level = "online" }}
                "???" = {{ url = "ws://refuse-the-r.ing", check-level = "none" }}
            "# },
            static_dir.path().display()
        ))
        .unwrap()
    }

    #[expect(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_webring() {
        let config = make_config();
        let webring = Webring::new(&config);

        {
            let inner = webring.members.read().unwrap();
            let mut expected = IndexMap::new();
            expected.insert(
                Intern::new("hrovnyak.gitlab.io".parse().unwrap()),
                Member {
                    name: "henry".to_owned(),
                    website: Intern::new(Uri::from_static("hrovnyak.gitlab.io")),
                    authority: Intern::new("hrovnyak.gitlab.io".parse().unwrap()),
                    discord_id: Some("123".parse().unwrap()),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                    last_notified: Arc::new(AsyncMutex::new(None)),
                },
            );
            expected.insert(
                Intern::new("kasad.com".parse().unwrap()),
                Member {
                    name: "kian".to_owned(),
                    website: Intern::new(Uri::from_static("kasad.com")),
                    authority: Intern::new("kasad.com".parse().unwrap()),
                    discord_id: Some("456".parse().unwrap()),
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                    last_notified: Arc::new(AsyncMutex::new(None)),
                },
            );
            expected.insert(
                Intern::new("localhost:32751".parse().unwrap()),
                Member {
                    name: "cynthia".to_owned(),
                    website: Intern::new(Uri::from_static("https://localhost:32751")),
                    authority: Intern::new("localhost:32751".parse().unwrap()),
                    discord_id: Some("789".parse().unwrap()),
                    check_level: CheckLevel::JustOnline,
                    check_successful: Arc::new(AtomicBool::new(true)),
                    last_notified: Arc::new(AsyncMutex::new(None)),
                },
            );
            expected.insert(
                Intern::new("refuse-the-r.ing".parse().unwrap()),
                Member {
                    name: "???".to_owned(),
                    website: Intern::new(Uri::from_static("ws://refuse-the-r.ing")),
                    authority: Intern::new("refuse-the-r.ing".parse().unwrap()),
                    discord_id: None,
                    check_level: CheckLevel::None,
                    check_successful: Arc::new(AtomicBool::new(true)),
                    last_notified: Arc::new(AsyncMutex::new(None)),
                },
            );
            assert_eq!(*inner, expected);
        }

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        webring.assert_next(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("kasad.com"),
        );
        webring.assert_stat_entry(
            (
                today,
                "hrovnyak.gitlab.io",
                "kasad.com",
                "hrovnyak.gitlab.io",
            ),
            1,
        );

        webring.assert_prev(
            "https://hrovnyak.gitlab.io/bruh/bruh/bruh?bruh=bruh",
            Ok("ws://refuse-the-r.ing"),
        );
        webring.assert_stat_entry(
            (
                today,
                "hrovnyak.gitlab.io",
                "refuse-the-r.ing",
                "hrovnyak.gitlab.io",
            ),
            1,
        );

        webring.assert_next("huh://refuse-the-r.ing", Ok("hrovnyak.gitlab.io"));
        webring.assert_prev(
            "https://kasad.com:3000",
            Err(TraverseWebringError::AuthorityNotFound(
                Authority::from_static("kasad.com:3000"),
            )),
        );
        webring.assert_next("https://kasad.com", Ok("https://localhost:32751"));
        webring.assert_prev(
            "/relative/uri",
            Err(TraverseWebringError::NoAuthority(Uri::from_static(
                "/relative/uri",
            ))),
        );

        // Henry is failing a check??
        webring.members.write().unwrap()[0]
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
            found_in_random.insert(
                (*webring
                    .random_page(Some(&origin), "0.0.0.0".parse().unwrap())
                    .unwrap())
                .clone(),
            );
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://localhost:32751"));
        assert_eq!(found_in_random, expected_random);

        // Test random without a specified origin
        found_in_random.clear();
        for _ in 0..200 {
            found_in_random.insert(
                (*webring
                    .random_page(None, "0.0.0.0".parse().unwrap())
                    .unwrap())
                .clone(),
            );
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://localhost:32751"));
        expected_random.insert(Uri::from_static("ws://refuse-the-r.ing"));
        assert_eq!(found_in_random, expected_random);

        // Test random with a specified origin not matching any site
        let origin = Uri::from_static("https://ring.purduehackers.com");
        found_in_random.clear();
        for _ in 0..200 {
            found_in_random.insert(
                (*webring
                    .random_page(Some(&origin), "0.0.0.0".parse().unwrap())
                    .unwrap())
                .clone(),
            );
        }
        let mut expected_random = HashSet::new();
        expected_random.insert(Uri::from_static("kasad.com"));
        expected_random.insert(Uri::from_static("https://localhost:32751"));
        expected_random.insert(Uri::from_static("ws://refuse-the-r.ing"));
        assert_eq!(found_in_random, expected_random);

        let new_members: IndexMap<String, MemberSpec> = toml::from_str(indoc! { r#"
            cynthia = { url = "https://localhost:32751", discord-id = 789, check-level = "links" }
            henry = { url = "hrovnyak.gitlab.io", discord-id = 123, check-level = "none" }
            "???" = { url = "http://refuse-the-r.ing", discord-id = 293847, check-level = "none" }
            arhan = { url = "arhan.sh", check-level = "none" }
            kian = { url = "kasad.com", discord-id = 456, check-level = "none" }
        "#})
        .unwrap();

        webring.update_members_and_check(&new_members).await;

        // Make sure that it found that Cynthia's links aren't there
        assert_eq!(
            webring.members.read().unwrap()[0]
                .check_successful
                .load(Ordering::Relaxed),
            false
        );

        // Make sure that it didn't re-check Henry because he didn't change
        assert_eq!(
            webring.members.read().unwrap()[1]
                .check_successful
                .load(Ordering::Relaxed),
            false
        );

        webring.members.read().unwrap()[0]
            .check_successful
            .store(true, Ordering::Relaxed);

        webring.assert_next("https://localhost:32751", Ok("http://refuse-the-r.ing"));
        webring.assert_next("hrovnyak.gitlab.io", Ok("http://refuse-the-r.ing"));
        webring.assert_next("refuse-the-r.ing", Ok("arhan.sh"));
        webring.assert_next("arhan.sh", Ok("kasad.com"));
        webring.assert_next("kasad.com", Ok("https://localhost:32751"));

        for i in 0..5 {
            webring.members.read().unwrap()[i]
                .check_successful
                .store(false, Ordering::Relaxed);
        }

        webring.assert_next("kasad.com", Err(TraverseWebringError::AllMembersFailing));
        webring.assert_prev(
            "https://localhost:32751",
            Err(TraverseWebringError::AllMembersFailing),
        );
        assert_eq!(
            webring.random_page(None, "0.0.0.0".parse().unwrap()),
            Err(TraverseWebringError::AllMembersFailing)
        );
        webring.assert_prev(
            "https://localhost:32751",
            Err(TraverseWebringError::AllMembersFailing),
        );

        webring.members.write().unwrap()[3]
            .check_successful
            .store(true, Ordering::Relaxed);

        for _ in 0..200 {
            assert_eq!(
                webring.random_page(None, "0.0.0.0".parse().unwrap()),
                Ok(Intern::new(Uri::from_static("arhan.sh")))
            );
        }

        webring.assert_prev("arhan.sh", Err(TraverseWebringError::AllMembersFailing));
        webring.assert_next("arhan.sh", Err(TraverseWebringError::AllMembersFailing));

        webring.assert_prev("refuse-the-r.ing", Ok("arhan.sh"));
        webring.assert_next("kasad.com", Ok("arhan.sh"));
    }

    #[tokio::test]
    async fn test_random_stats_unknown_origin() {
        let mut config = make_config();
        config.members = toml::from_str(indoc! { r#"
            [kian]
            url = "kasad.com"
            discord-id = 456
            check-level = "none"
        "# })
        .unwrap();
        let webring = Webring::new(&config);

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        let uri = webring
            .random_page(None, "0.0.0.0".parse().unwrap())
            .unwrap();
        assert_eq!(uri, Intern::new("kasad.com".parse().unwrap()));
        webring.assert_stat_entry(
            (
                today,
                UNKNOWN_ORIGIN.as_str(),
                "kasad.com",
                UNKNOWN_ORIGIN.as_str(),
            ),
            1,
        );
    }

    #[tokio::test]
    async fn test_random_stats() {
        let mut config = make_config();
        config.members = toml::from_str(indoc!{ r#"
            kian = { url = "kasad.com", discord-id = 456, check-level = "none" }
            cynthia = { url = "https://clementine.viridian.page", discord-id = 789, check-level = "none" }
        "# }).unwrap();
        let webring = Webring::new(&config);

        let today = Utc::now().with_timezone(&TIMEZONE).date_naive();

        let uri = webring
            .random_page(
                Some(&"clementine.viridian.page".parse().unwrap()),
                "0.0.0.0".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(uri, Intern::new("kasad.com".parse().unwrap()));
        webring.assert_stat_entry(
            (
                today,
                "clementine.viridian.page",
                "kasad.com",
                "clementine.viridian.page",
            ),
            1,
        );
    }

    fn setup() -> (NamedTempFile, TempDir, Arc<Webring>) {
        let static_dir = TempDir::new().unwrap();
        File::create_new(static_dir.path().join("index.html")).unwrap();

        let mut config_file = NamedTempFile::new().unwrap();
        write!(
            config_file,
            indoc! { r#"
            webring.static-dir = "{}"
            network.listen-addr = "0.0.0.0:3000"
            members.kian = {{ url = "https://kasad.com", check-level = "none" }}
        "# },
            static_dir.path().display()
        )
        .unwrap();
        config_file.flush().unwrap();

        let webring = Webring::new(
            &toml::from_str(&std::fs::read_to_string(config_file.path()).unwrap()).unwrap(),
        );

        (config_file, static_dir, Arc::new(webring))
    }

    #[tokio::test]
    async fn test_reload_config() {
        let (config_file, _static_dir, webring) = setup();
        webring.enable_reloading(config_file.path()).unwrap();
        assert_eq!(1, webring.members.read().unwrap().len());
        write!(
            OpenOptions::new()
                .append(true)
                .open(config_file.path())
                .unwrap(),
            r#"members.henry = {{ url = "https://hrovnyak.gitlab.io", check-level = "none" }}"#
        )
        .unwrap();
        // Wait for a bit just in case the event takes some time to process
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(2, webring.members.read().unwrap().len());
    }

    #[tokio::test]
    async fn test_reload_homepage() {
        let (config_file, static_dir, webring) = setup();
        webring.enable_reloading(config_file.path()).unwrap();

        // Generate the homepage
        webring.homepage().await.unwrap();
        assert!(webring.homepage.read().await.is_some());

        // Change the template
        tokio::fs::write(static_dir.path().join("index.html"), "test")
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
        let (config_file, _static_dir, webring) = setup();
        let weak_ptr = Arc::downgrade(&webring);
        webring.enable_reloading(config_file.path()).unwrap();
        drop(webring);
        assert!(weak_ptr.upgrade().is_none());
        assert_eq!(0, weak_ptr.strong_count());
        assert_eq!(0, weak_ptr.weak_count());
    }

    /// Test notification sending logic.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_notification_debounce() {
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        enum SiteStatus {
            Up,
            Down,
        }
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        enum ShouldNotify {
            Yes,
            No,
        }
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        enum NotifyStatus {
            Success,
            Fail,
        }

        let sequence = &[
            // site is up; no notification
            (SiteStatus::Up, ShouldNotify::No, None, None),
            // site is newly down; should notify
            (
                SiteStatus::Down,
                ShouldNotify::Yes,
                Some(NotifyStatus::Success),
                None,
            ),
            // site is still down; should not notify
            (
                SiteStatus::Down,
                ShouldNotify::No,
                None,
                Some(NOTIFICATION_DEBOUNCE_PERIOD + Duration::from_secs(10)),
            ),
            // waited 25 hours; should notify again
            (
                SiteStatus::Down,
                ShouldNotify::Yes,
                Some(NotifyStatus::Fail),
                None,
            ),
            // last notification failed, so didn't count; should notify again
            (
                SiteStatus::Down,
                ShouldNotify::Yes,
                Some(NotifyStatus::Success),
                None,
            ),
            // site is still down; should not notify
            (SiteStatus::Down, ShouldNotify::No, None, None),
        ];

        let server = MockServer::start();

        let site_url: Uri = server.url("/site").parse().unwrap();
        let member = Member {
            name: "bad".to_owned(),
            website: Intern::new(site_url.clone()),
            authority: Intern::new(site_url.into_parts().authority.unwrap()),
            discord_id: Some(Snowflake::new(1234)),
            check_level: CheckLevel::JustOnline,
            check_successful: Arc::new(AtomicBool::new(true)),
            last_notified: Arc::new(AsyncMutex::new(None)),
        };

        let notifier = Arc::new(DiscordNotifier::new(
            &server.url("/discord").parse().unwrap(),
        ));
        let base_address = Intern::new(Uri::from_static("https://ring.purduehackers.com"));

        for (i, &(site_status, should_notify, notify_status, delay)) in sequence.iter().enumerate()
        {
            // Create mock endpoints
            let mut mock_bad_site = server.mock(|when, then| {
                when.path("/site");
                // Respond with the next status in the sequence
                then.status(match site_status {
                    SiteStatus::Up => 200,
                    SiteStatus::Down => 404,
                });
            });

            // Mocks the Discord notification endpoint; succeeds/fails based on the sequence.
            let mut mock_discord_server = server.mock(|when, then| {
                when.method(POST).path("/discord");
                then.status(match notify_status {
                    Some(NotifyStatus::Success) => 204,
                    Some(NotifyStatus::Fail) => 500,
                    _ => 404,
                });
            });

            // Perform check
            let maybe_notification_task = member
                .check_and_store_and_optionally_notify(base_address, Some(Arc::clone(&notifier)))
                .await;
            if let Some(notification_task) = maybe_notification_task {
                // Wait for the notification task to complete
                notification_task.await.unwrap();
            }
            match should_notify {
                ShouldNotify::No => {
                    assert_eq!(
                        mock_discord_server.hits(),
                        0,
                        "expected no notification for step {i}"
                    );
                }
                ShouldNotify::Yes => {
                    assert_eq!(
                        mock_discord_server.hits(),
                        1,
                        "expected notification for step {i}"
                    );
                }
            }
            // Check stored status
            assert_eq!(
                member.check_successful.load(Ordering::Relaxed),
                site_status == SiteStatus::Up,
                "check_successful mismatch at step {i}"
            );
            if let Some(delay) = delay {
                tokio::time::pause();
                tokio::time::advance(delay).await;
                tokio::time::resume();
            }

            // Delete mock endpoints
            mock_bad_site.delete();
            mock_discord_server.delete();
        }
    }
}
