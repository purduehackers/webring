use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use axum::http::Uri;
use sarlacc::Intern;

use super::{Screenshotter, WebpScreenshotData};

/// Dummy screenshotter for unit tests. Has an increasing counter and returns
/// screenshot data which consists of the UTF-8 encoding of the text `webp
/// screenshot N`, where `N` is the counter value and increases for each
/// screenshot taken.
#[derive(Clone, Debug)]
pub struct TestScreenshotter {
    /// Increasing counter used to assign unique numbers to returned screenshots
    counter: Arc<AtomicU64>,
}

impl TestScreenshotter {
    /// Creates a test screenshotter.
    pub fn new() -> TestScreenshotter {
        TestScreenshotter {
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns the number of screenshots taken so far.
    pub fn screenshots_taken(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }
}

impl Screenshotter for TestScreenshotter {
    fn take_screenshot(
        &self,
        _site: Intern<Uri>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<WebpScreenshotData>> + Send + Sync + 'static>>
    {
        Box::pin(std::future::ready(Ok(WebpScreenshotData(
            format!(
                "webp screenshot {}",
                self.counter.fetch_add(1, Ordering::Relaxed)
            )
            .into_bytes(),
        ))))
    }
}
