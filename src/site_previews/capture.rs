//! Capture member website previews with Chromium.

use std::{
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use axum::http::Uri;
use chromiumoxide::{
    Browser, BrowserConfig,
    cdp::browser_protocol::{
        page::{CaptureScreenshotFormat, CaptureScreenshotParams},
        target::CreateTargetParams,
    },
    handler::viewport::Viewport,
    page::ScreenshotParams,
};
use eyre::{Context, Report};
use futures::StreamExt;
use sarlacc::Intern;
use tokio::{
    sync::{mpsc, oneshot},
    task::AbortHandle,
};
use tracing::{error, info, instrument};

/// Viewport for Chromium screenshots
const VIEWPORT: Viewport = Viewport {
    width: 1440,
    height: 910,
    is_landscape: true,
    device_scale_factor: None,
    emulating_mobile: false,
    has_touch: false,
};

/// Typed wrapper for WebP image data.
#[derive(Debug)]
pub struct WebpScreenshotData(pub Vec<u8>);

/// Represents a queued screenshot-taking request.
#[derive(Debug)]
pub struct TakeScreenshotJob {
    /// URI of the site to take a screenshot of.
    pub site: Intern<Uri>,
    /// Write end of a channel on which the result should be submitted once done.
    pub result: oneshot::Sender<eyre::Result<WebpScreenshotData>>,
}

/// Interface implemented by objects which can take screenshots of sites.
pub trait Screenshotter: Debug + Send + Sync {
    /// Enqueues a screenshot-taking job for the screenshotter to process at its
    /// discretion.
    fn enqueue_job(&self, job: TakeScreenshotJob);
}

/// Screenshotter which uses Chromium via [`chromiumoxide`].
#[derive(Debug)]
pub struct ChromiumScreenshotter {
    /// Write end of channel used to submit jobs to the screenshot processor task.
    jobs: tokio::sync::mpsc::UnboundedSender<TakeScreenshotJob>,
    /// Handles on the background tasks that are part of this screenshotter.
    tasks: [AbortHandle; 2],
}

impl ChromiumScreenshotter {
    /// Creates a new Chromium-based screenshotter.
    ///
    /// This launches a headless Chromium browser instance in the background.
    pub async fn new() -> eyre::Result<ChromiumScreenshotter> {
        let config = BrowserConfig::builder()
            .viewport(VIEWPORT.clone())
            .arg("--hide-scrollbars")
            .build()
            .map_err(Report::msg)
            .wrap_err("failed to create browser configuration")?;
        let (browser, handler) = Browser::launch(config)
            .await
            .wrap_err("failed to launch Chromium browser")?;

        // Spawn event handler task
        let browser_event_handler = tokio::task::spawn(async move {
            let mut handler = handler;
            while let Some(event) = handler.next().await {
                if let Err(err) = event {
                    error!(%err, "Chromium CDP error");
                }
            }
        });

        let (jobs, receiver) = mpsc::unbounded_channel();

        // Spawn processor task
        let processor = tokio::task::spawn(ChromiumScreenshotter::run(browser, receiver));

        Ok(ChromiumScreenshotter {
            jobs,
            tasks: [
                browser_event_handler.abort_handle(),
                processor.abort_handle(),
            ],
        })
    }

    /// Runs the processing loop which reads jobs from the queue and handles
    /// them using the browser.
    async fn run(browser: Browser, mut receiver: mpsc::UnboundedReceiver<TakeScreenshotJob>) {
        while let Some(job) = receiver.recv().await {
            let result = ChromiumScreenshotter::capture_screenshot(&browser, job.site).await;
            // We don't care if the caller is no longer waiting for the result
            let _ = job.result.send(result);
        }
    }

    /// Captures a screenshot for a single site.
    ///
    /// Opens a new tab in the given browser, loads the given site, screenshots
    /// it, and closes the tab when done.
    #[instrument]
    async fn capture_screenshot(
        browser: &Browser,
        site: Intern<Uri>,
    ) -> eyre::Result<WebpScreenshotData> {
        let page_params = CreateTargetParams::builder()
            .url(site.to_string())
            .build()
            // Only missing `url` causes an error, so we won't get one
            .expect("invalid browser page parameters");
        // Creating a new page will wait until the load event fires
        let page_result =
            tokio::time::timeout(Duration::from_secs(10), browser.new_page(page_params))
                .await
                .wrap_err("timed out opening site in new browser page")?;
        let page = page_result.wrap_err("failed to create browser page")?;
        let image_data_result = async {
            let screenshot_params = ScreenshotParams {
                cdp_params: CaptureScreenshotParams {
                    format: Some(CaptureScreenshotFormat::Webp),
                    ..CaptureScreenshotParams::default()
                },
                ..ScreenshotParams::default()
            };
            page.screenshot(screenshot_params)
                .await
                .map(WebpScreenshotData)
                .wrap_err("failed to take screenshot")
        }
        .await;
        // Close the page regardless of success/failure
        let _ = page.close().await;
        info!(?site, "captured screenshot of site");
        image_data_result
    }
}

impl Screenshotter for ChromiumScreenshotter {
    fn enqueue_job(&self, job: TakeScreenshotJob) {
        let _ = self.jobs.send(job);
    }
}

impl Drop for ChromiumScreenshotter {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        // Since the tasks own the [Browser], it will be dropped when they abort.
    }
}

/// Dummy screenshotter for unit tests. Has an increasing counter and returns
/// screenshot data which consists of the UTF-8 encoding of the text `webp
/// screenshot N`, where `N` is the counter value and increases for each
/// screenshot taken.
#[derive(Debug)]
pub struct TestScreenshotter {
    /// Increasing counter used to assign unique numbers to returned screenshots
    counter: AtomicU64,
}

impl TestScreenshotter {
    /// Creates a test screenshotter.
    pub fn new() -> TestScreenshotter {
        TestScreenshotter {
            counter: AtomicU64::new(0),
        }
    }
}

impl Screenshotter for TestScreenshotter {
    fn enqueue_job(&self, job: TakeScreenshotJob) {
        job.result
            .send(Ok(WebpScreenshotData(
                format!(
                    "webp screenshot {}",
                    self.counter.fetch_add(1, Ordering::Relaxed)
                )
                .into_bytes(),
            )))
            .unwrap();
    }
}
