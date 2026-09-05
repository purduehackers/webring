//! Generate static website previews with Playwright's Chromium CLI.

use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use clap::Parser;
use serde::Deserialize;

const VIEWPORT: &str = "1440,910";
const PLAYWRIGHT_VERSION: &str = "1.52.0";

#[derive(Debug, Parser)]
#[command(about = "Capture webring member website previews")]
struct Options {
    /// Path to the webring configuration file.
    #[arg(short = 'f', long, default_value = "webring.toml")]
    config_file: PathBuf,

    /// Directory in which to write preview PNGs.
    #[arg(long, default_value = "static/previews")]
    output_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    members: BTreeMap<String, Member>,
}

#[derive(Debug, Deserialize)]
struct Member {
    #[serde(alias = "site")]
    url: String,
}

fn preview_id(name: &str) -> String {
    let id: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();

    if id.is_empty() {
        "member".to_owned()
    } else {
        id
    }
}

fn absolute_url(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

fn capture(url: &str, output: &Path) -> Result<bool, Box<dyn Error>> {
    let status = Command::new("npx")
        .args([
            "--yes",
            &format!("playwright@{PLAYWRIGHT_VERSION}"),
            "screenshot",
            "--browser=chromium",
            "--viewport-size",
            VIEWPORT,
            "--wait-for-timeout",
            "1500",
            url,
        ])
        .arg(output)
        .status()?;

    Ok(status.success())
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse();
    let config: Config = toml::from_str(&fs::read_to_string(&options.config_file)?)?;
    fs::create_dir_all(&options.output_dir)?;

    let mut failures = 0;
    for (name, member) in config.members {
        let output = options
            .output_dir
            .join(format!("{}.png", preview_id(&name)));
        let url = absolute_url(&member.url);
        eprintln!("Capturing {name} ({url})");

        if !capture(&url, &output)? {
            failures += 1;
            eprintln!("Failed to capture {name}; keeping the previous preview if present");
        }
    }

    if failures == 0 {
        Ok(())
    } else {
        Err(format!("{failures} preview(s) failed").into())
    }
}

#[cfg(test)]
mod tests {
    use super::{absolute_url, preview_id};

    #[test]
    fn preview_ids_are_safe() {
        assert_eq!("member-name", preview_id("member/name"));
        assert_eq!("member", preview_id(""));
    }

    #[test]
    fn urls_default_to_https() {
        assert_eq!("https://example.com", absolute_url("example.com"));
        assert_eq!("http://example.com", absolute_url("http://example.com"));
    }
}
