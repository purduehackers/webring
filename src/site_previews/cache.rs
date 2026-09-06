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
        Ok(SitePreviewCache {
            cache_dir: config.webring.cache_dir.clone(),
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
            None => todo!("return placeholder image"),
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
