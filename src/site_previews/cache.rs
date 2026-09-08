//! Site screenshot cache

use std::{
    fmt::{Display, Formatter},
    io::ErrorKind,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::http::Uri;
use eyre::WrapErr;
use papaya::HashSet;
use sarlacc::Intern;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

use crate::{
    config::Config,
    site_previews::capture::{Screenshotter, TakeScreenshotJob, WebpScreenshotData},
};

/// A filename-safe ID to represent a site preview.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SitePreviewId(String);

impl Display for SitePreviewId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl SitePreviewId {
    /// Creates a site preview ID from a webring member's name.
    pub fn from_name(name: &str) -> SitePreviewId {
        SitePreviewId(
            name.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect(),
        )
    }
}

/// Site preview response.
#[derive(Debug)]
pub struct SitePreview {
    /// The screenshot image data.
    pub image: WebpScreenshotData,
    /// The time at which the cached preview expires. May be in the past.
    pub expires_at: SystemTime,
}

/// A cache for site previews.
///
/// This cache stores the preview screenshots in the filesystem. It uses its
/// [`screenshotter`] to generate new screenshots when cached ones expire.
#[derive(Debug)]
pub struct SitePreviewCache {
    /// Directory in which cached screenshots are stored
    cache_dir: PathBuf,
    /// Path to the image returned when no cached screenshot is available
    placeholder_path: PathBuf,
    /// Screenshotter used to take screenshots of sites
    screenshotter: Box<dyn Screenshotter>,
    /// Set which tracks which sites are currently being revalidated. Used to
    /// deduplicate requests for revalidation.
    revalidating: Arc<HashSet<SitePreviewId>>,
    /// The amount of time cached screenshots are considered fresh for. After
    /// this time, they will be revalidated.
    revalidation_period: Duration,
}

impl SitePreviewCache {
    /// Creates a preview cache.
    ///
    /// The cache directory and revalidation period are sourced from `config`.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the cache directory fails.
    pub async fn new(
        config: &Config,
        screenshotter: Box<dyn Screenshotter>,
    ) -> eyre::Result<SitePreviewCache> {
        tokio::fs::create_dir_all(&config.webring.cache_dir)
            .await
            .wrap_err("failed to create screenshot cache directory")?;
        let placeholder_path = config
            .webring
            .static_dir
            .join("site_preview_placeholder.webp");
        Ok(SitePreviewCache {
            cache_dir: config.webring.cache_dir.clone(),
            placeholder_path,
            revalidation_period: config.webring.preview_cache_duration,
            screenshotter,
            revalidating: Arc::new(HashSet::new()),
        })
    }

    /// Fetches the preview screenshot for the given site from the cache. If the
    /// screenshot is stale, it is returned and a revalidation is requested in
    /// the background.
    ///
    /// This function always succeeds. If there is an error loading the
    /// screenshot from the cache, an error is logged and the placeholder image
    /// is returned.
    pub async fn get_preview(&self, id: &SitePreviewId, uri: Intern<Uri>) -> SitePreview {
        let path = self.target_path(id);
        let file = match tokio::fs::File::open(&path).await {
            Ok(file) => Some(file),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                error!(%err, ?path, "failed to open cached screenshot file");
                None
            }
        };
        let maybe_preview = match file {
            None => {
                warn!(%id, "no screenshot for site; returning placeholder");
                None
            }
            Some(mut file) => {
                let result: eyre::Result<SitePreview> = async {
                    let metadata = file
                        .metadata()
                        .await
                        .wrap_err("failed to stat screenshot file")?;
                    let mtime = metadata.modified().wrap_err("file mtime isn't available")?;
                    let expires_at = mtime + self.revalidation_period;
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)
                        .await
                        .wrap_err("failed to read cached screenshot file")?;
                    debug!(%id, "cached screenshot hit");
                    Ok(SitePreview {
                        image: WebpScreenshotData(buf),
                        expires_at,
                    })
                }
                .await;
                match result {
                    Ok(preview) => Some(preview),
                    Err(err) => {
                        error!(%err, ?path, "error loading screenshot from cached file");
                        None
                    }
                }
            }
        };
        if maybe_preview
            .as_ref()
            .is_none_or(|preview| preview.expires_at <= SystemTime::now())
        {
            self.revalidate(id, uri);
        }
        match maybe_preview {
            Some(preview) => preview,
            None => {
                // Here we are okay with unwrapping because it is an error if
                // there is no placeholder. The Axum server will catch the panic
                // and return a 500 error, which is what we'd do manually if we
                // returned a Result.
                let image = tokio::fs::read(&self.placeholder_path)
                    .await
                    .expect("failed to read site preview placeholder");
                SitePreview {
                    image: WebpScreenshotData(image),
                    expires_at: SystemTime::UNIX_EPOCH,
                }
            }
        }
    }

    /// Gets the image path for a given [`SitePreviewId`].
    fn target_path(&self, id: &SitePreviewId) -> PathBuf {
        self.cache_dir.join(format!("{}.webp", id.0))
    }

    /// Requests revalidation of the given site preview in the background.
    /// The request is submitted to the [`Screenshotter`] and a background task
    /// is spawned to wait for the result and save it in the cache directory.
    fn revalidate(&self, id: &SitePreviewId, uri: Intern<Uri>) {
        if !self.revalidating.pin().insert(id.clone()) {
            return;
        }
        info!(%id, ?uri, "requesting screenshot of site");
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.screenshotter.enqueue_job(TakeScreenshotJob {
            site: uri,
            result: tx,
        });
        let target_path = self.target_path(id);
        let revalidating = Arc::clone(&self.revalidating);
        let id = id.clone();
        tokio::task::spawn(async move {
            if let Ok(result) = rx.await {
                // The code in this block must not return early because removing
                // the revalidating marker happens afterwards.
                match result {
                    Ok(image) => {
                        if let Err(err) = tokio::fs::write(&target_path, &image.0).await {
                            error!(err = %format_args!("{err:#}"), ?target_path, "failed to save screenshot image");
                        }
                    }
                    Err(err) => {
                        error!(err = %format_args!("{err:#}"), %uri, "failed to take screenshot");
                    }
                }
            }
            revalidating.pin().remove(&id);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::LazyLock, time::Duration};

    use axum::http::Uri;
    use pretty_assertions::assert_eq;
    use sarlacc::Intern;
    use tempfile::TempDir;
    use tokio::{fs, time::timeout};

    use crate::{
        config::{Config, WebringTable},
        site_previews::{SitePreviewCache, SitePreviewId, TestScreenshotter},
    };

    static URI: LazyLock<Intern<Uri>> =
        LazyLock::new(|| Intern::new(Uri::from_static("https://example.com")));

    fn test_config(cache_dir: &Path, static_dir: &Path, cache_duration: Duration) -> Config {
        Config {
            webring: WebringTable {
                cache_dir: cache_dir.to_owned(),
                static_dir: static_dir.to_owned(),
                preview_cache_duration: cache_duration,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    async fn make_cache(
        revalidation_period: Duration,
    ) -> (TempDir, SitePreviewCache, SitePreviewId, TestScreenshotter) {
        let temp_dir = TempDir::new().unwrap();
        let static_dir = temp_dir.path().join("static");
        fs::create_dir(&static_dir).await.unwrap();
        fs::write(
            static_dir.join("site_preview_placeholder.webp"),
            b"placeholder image",
        )
        .await
        .unwrap();
        let config = test_config(
            &temp_dir.path().join("cache"),
            &static_dir,
            revalidation_period,
        );
        let screenshotter = TestScreenshotter::new();
        let cache = SitePreviewCache::new(&config, Box::new(screenshotter.clone()))
            .await
            .unwrap();
        let id = SitePreviewId::from_name("test member");
        (temp_dir, cache, id, screenshotter)
    }

    async fn wait_for_cached_image(path: &Path, expected: &[u8]) {
        timeout(Duration::from_secs(1), async {
            loop {
                if fs::read(path)
                    .await
                    .is_ok_and(|contents| contents == expected)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for cached screenshot");
    }

    #[test]
    fn preview_ids_are_filename_safe() {
        assert_eq!(
            "letters-AND_0123------",
            SitePreviewId::from_name("letters-AND_0123 /..!?").to_string()
        );
    }

    #[tokio::test]
    async fn creates_cache_directory() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("nested/cache");
        let config = test_config(&cache_dir, temp_dir.path(), Duration::from_mins(1));
        assert!(!cache_dir.exists());
        SitePreviewCache::new(&config, Box::new(TestScreenshotter::new()))
            .await
            .unwrap();
        assert!(cache_dir.is_dir());
    }

    #[tokio::test]
    async fn returns_fresh_cached_preview() {
        let (_temp_dir, cache, id, screenshotter) = make_cache(Duration::from_hours(1)).await;
        let path = cache.target_path(&id);
        fs::write(&path, b"cached screenshot").await.unwrap();
        let preview = cache.get_preview(&id, *URI).await;
        assert_eq!(b"cached screenshot", preview.image.0.as_slice());
        assert!(preview.expires_at > std::time::SystemTime::now());
        assert_eq!(0, screenshotter.screenshots_taken());
    }

    #[tokio::test]
    async fn returns_placeholder_when_preview_is_not_cached() {
        let (temp_dir, cache, id, screenshotter) = make_cache(Duration::from_hours(1)).await;

        let first_preview = cache.get_preview(&id, *URI).await;
        assert_eq!(b"placeholder image", first_preview.image.0.as_slice());

        fs::write(
            temp_dir.path().join("static/site_preview_placeholder.webp"),
            b"updated placeholder image",
        )
        .await
        .unwrap();
        let other_id = SitePreviewId::from_name("another member");
        let second_preview = cache.get_preview(&other_id, *URI).await;

        assert_eq!(
            b"updated placeholder image",
            second_preview.image.0.as_slice()
        );
        assert!(second_preview.expires_at <= std::time::SystemTime::now());
        assert_eq!(2, screenshotter.screenshots_taken());
        wait_for_cached_image(&cache.target_path(&id), b"webp screenshot 0").await;
        wait_for_cached_image(&cache.target_path(&other_id), b"webp screenshot 1").await;
    }

    #[tokio::test]
    async fn returns_stale_preview_while_revalidating_it() {
        let (_temp_dir, cache, id, screenshotter) = make_cache(Duration::ZERO).await;
        let path = cache.target_path(&id);
        fs::write(&path, b"stale screenshot").await.unwrap();
        let preview = cache.get_preview(&id, *URI).await;
        assert_eq!(b"stale screenshot", preview.image.0.as_slice());
        wait_for_cached_image(&path, b"webp screenshot 0").await;
        assert_eq!(1, screenshotter.screenshots_taken());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deduplicates_concurrent_revalidations() {
        let (_temp_dir, cache, id, screenshotter) = make_cache(Duration::ZERO).await;
        // Since we're using the single-threaded runtime, these two calls will
        // complete before the result gets processed by the background task.
        cache.revalidate(&id, *URI);
        cache.revalidate(&id, *URI);
        assert_eq!(1, screenshotter.screenshots_taken());
        wait_for_cached_image(&cache.target_path(&id), b"webp screenshot 0").await;
    }
}
