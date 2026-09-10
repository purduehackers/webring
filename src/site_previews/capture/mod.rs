//! Screenshot capture implementations

mod chromium;
mod fallback;
#[cfg(test)]
mod test;

#[cfg(test)]
pub use test::TestScreenshotter;

pub use chromium::ChromiumScreenshotter;
pub use fallback::FallbackImageGenerator;

use axum::http::Uri;
use sarlacc::Intern;
use std::{fmt::Debug, pin::Pin};

/// Typed wrapper for WebP image data.
#[derive(Debug)]
pub struct WebpScreenshotData(pub Vec<u8>);

/// Interface implemented by objects which can take screenshots of sites.
pub trait Screenshotter: Debug + Send + Sync {
    /// Enqueues a screenshot-taking job for the screenshotter to process at its
    /// discretion.
    fn take_screenshot(
        &self,
        site: Intern<Uri>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<WebpScreenshotData>> + Send + Sync + 'static>>;
}

/// Width for site screenshot images
const WIDTH: u16 = 960;
/// Height for site screenshot images
const HEIGHT: u16 = 608;
