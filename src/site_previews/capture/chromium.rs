/*
Copyright (C) 2025 Kian Kasad

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

//! Capture member website previews with Chromium.

use std::{fmt::Debug, pin::Pin, time::Duration};

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

use crate::site_previews::capture::{Screenshotter, WebpScreenshotData};

/// Viewport for Chromium screenshots
const VIEWPORT: Viewport = Viewport {
    width: super::WIDTH as u32,
    height: super::HEIGHT as u32,
    is_landscape: super::WIDTH >= super::HEIGHT,
    device_scale_factor: None,
    emulating_mobile: false,
    has_touch: false,
};

/// Represents a queued screenshot-taking request.
#[derive(Debug)]
struct Job {
    /// URI of the site to take a screenshot of.
    pub site: Intern<Uri>,
    /// Write end of a channel on which the result should be submitted once done.
    pub result: oneshot::Sender<eyre::Result<WebpScreenshotData>>,
}

/// Screenshotter which uses Chromium via [`chromiumoxide`].
#[derive(Debug)]
pub struct ChromiumScreenshotter {
    /// Write end of channel used to submit jobs to the screenshot processor task.
    jobs: tokio::sync::mpsc::UnboundedSender<Job>,
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
    async fn run(browser: Browser, mut receiver: mpsc::UnboundedReceiver<Job>) {
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
    fn take_screenshot(
        &self,
        site: Intern<Uri>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<WebpScreenshotData>> + Send + Sync + 'static>>
    {
        let (tx, rx) = oneshot::channel();
        let submit_result = self
            .jobs
            .send(Job { site, result: tx })
            .wrap_err("screenshotter processor task has died");
        Box::pin(async move {
            let () = submit_result?;
            rx.await.wrap_err("screenshotter processor task has died")?
        })
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

#[cfg(test)]
mod tests {
    use axum::{Router, http::Uri, routing::get};
    use sarlacc::Intern;

    use super::{ChromiumScreenshotter, Screenshotter};

    #[tokio::test]
    #[ignore = "requires Chromium to be installed"]
    async fn chromium_screenshotter_captures_webp() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/", get(async || "test page")),
            )
            .await
            .unwrap();
        });
        let screenshotter = ChromiumScreenshotter::new().await.unwrap();
        let site = Intern::new(format!("http://{address}").parse::<Uri>().unwrap());

        let image = screenshotter.take_screenshot(site).await.unwrap();

        assert_eq!(b"RIFF", &image.0[..4]);
        assert_eq!(b"WEBP", &image.0[8..12]);
        server.abort();
    }
}
