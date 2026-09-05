//! Capture member website previews with Playwright's Chromium CLI.

use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use eyre::WrapErr;

/// Browser viewport used for member previews.
const VIEWPORT: &str = "1440,910";
/// Counter used to make temporary output paths unique within this process.
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// Capture a website without blocking the async runtime.
pub async fn capture(url: &str) -> eyre::Result<Vec<u8>> {
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || capture_blocking(&url)).await?
}

/// Capture a screenshot in a blocking worker and return its bytes.
fn capture_blocking(url: &str) -> eyre::Result<Vec<u8>> {
    let output = std::env::temp_dir().join(format!(
        "ph-webring-preview-{}-{}.png",
        std::process::id(),
        NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| {
        let status = Command::new("playwright")
            .args([
                "screenshot",
                "--browser=chromium",
                "--viewport-size",
                VIEWPORT,
                "--wait-for-timeout",
                "1500",
                url,
            ])
            .arg(&output)
            .status()
            .wrap_err("failed to run Playwright")?;

        if !status.success() {
            eyre::bail!("Playwright exited with {status}");
        }

        fs::read(&output).wrap_err("failed to read Playwright screenshot")
    })();
    let _ = fs::remove_file(output);
    result
}

/// Return an absolute URL for a URI that may omit its scheme.
pub fn absolute_url(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::absolute_url;

    #[test]
    fn urls_default_to_https() {
        assert_eq!("https://example.com", absolute_url("example.com"));
        assert_eq!("http://example.com", absolute_url("http://example.com"));
    }
}
