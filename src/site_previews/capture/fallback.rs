/*
Copyright (C) 2025 Kian Kasad and Amber Zeng

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

//! Placeholder screenshot image generation

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    pin::Pin,
};

use axum::http::Uri;
use palette::{FromColor, Hsl, Srgb};
use sarlacc::Intern;
use webp::Encoder;

use crate::site_previews::capture::{Screenshotter, WebpScreenshotData};

/// Generate a small, deterministic WebP fallback from a member name.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn fallback_image(key: &impl Hash, width: u32, height: u32) -> WebpScreenshotData {
    const CELL: u64 = 8;
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let key_hash = hasher.finish();
    // Generate a member-specific palette from dark to light.
    // let hue = f32::from(u16::try_from(id_hash % 360).unwrap());
    let hue = f32::from((key_hash % 360) as u16);
    let palette = [
        hsl_to_rgb(hue, 0.65, 0.08),
        hsl_to_rgb(hue, 0.55, 0.42),
        hsl_to_rgb(hue, 0.45, 0.88),
    ];
    let phase = key_hash % 4;
    let reverse = key_hash & 1 == 1;
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);

    for y in 0..u64::from(height) {
        for x in 0..u64::from(width) {
            let cell_x = x / CELL;
            let cell_y = y / CELL;
            // Quantize the gradient to whole dither cells so no transition cuts
            // through a cell and creates half-pixels.
            let vertical =
                cell_y as f32 / (u64::from(height) / CELL).saturating_sub(1).max(1) as f32;
            let gradient = if reverse { vertical } else { 1.0 - vertical };
            let scaled = gradient * (palette.len() - 1) as f32;
            let lower = (scaled as usize).min(palette.len() - 1);
            let upper = (lower + 1).min(palette.len() - 1);
            let blend = scaled - lower as f32;
            let threshold =
                f32::from(BAYER[((cell_y + phase) % 4) as usize][((cell_x + phase) % 4) as usize])
                    / 16.0;
            let color = if upper != lower && threshold < blend {
                palette[upper]
            } else {
                palette[lower]
            };
            pixels.extend_from_slice(&color);
        }
    }

    WebpScreenshotData(
        Encoder::from_rgb(&pixels, width, height)
            .encode(90.0)
            .to_vec(),
    )
}

/// Convert an HSL color (float) to an RGB triplet (u8).
fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [u8; 3] {
    let hsl = Hsl::new_srgb(hue, saturation, lightness);
    let rgb = Srgb::from_color(hsl).into_format::<u8>();
    palette::cast::into_array(rgb)
}

/// Fallback image generator.
///
/// Generates fallback images for sites in case we don't have a screenshot image
/// available to use.
#[derive(Debug)]
pub struct FallbackImageGenerator {
    /// Width of generated fallback images
    width: u32,
    /// Height of generated fallback images
    height: u32,
}

impl FallbackImageGenerator {
    /// Creates a fallback image generator with the given output dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl Screenshotter for FallbackImageGenerator {
    fn take_screenshot(
        &self,
        site: Intern<Uri>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<WebpScreenshotData>> + Send + Sync + 'static>>
    {
        Box::pin(std::future::ready(Ok(fallback_image(
            &*site,
            self.width,
            self.height,
        ))))
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use axum::http::Uri;
    use pretty_assertions::assert_ne;
    use sarlacc::Intern;
    use webp::Decoder;

    use crate::site_previews::capture::Screenshotter;

    use super::FallbackImageGenerator;

    #[tokio::test]
    async fn placeholder_images_are_unique() {
        let uri1 = Intern::new(Uri::from_static("https://kasad.com"));
        let uri2 = Intern::new(Uri::from_static("https://amberzeng.com"));
        let generator = FallbackImageGenerator::new(1280, 808);
        assert_ne!(
            generator.take_screenshot(uri1).await.unwrap().0,
            generator.take_screenshot(uri2).await.unwrap().0
        );
    }

    #[tokio::test]
    async fn placeholder_uses_configured_dimensions() {
        let uri = Intern::new(Uri::from_static("https://example.com"));
        let image = FallbackImageGenerator::new(64, 48)
            .take_screenshot(uri)
            .await
            .unwrap();
        let decoded = Decoder::new(&image.0).decode().unwrap();

        assert_eq!(64, decoded.width());
        assert_eq!(48, decoded.height());
    }

    #[tokio::test]
    #[ignore = "this test generates images in /tmp/images for manual inspection to guarantee the colors differ"]
    async fn dummy() {
        let generator = FallbackImageGenerator::new(1280, 808);
        for domain in [
            "kasad.com",
            "ericswpark.com",
            "rayhanadev.com",
            "kimjammer.com",
            "neels.page",
        ] {
            let uri = Intern::new(Uri::from_str(&format!("https://{domain}")).unwrap());
            let image = generator.take_screenshot(uri).await.unwrap();
            let path = PathBuf::from(format!("/tmp/images/{domain}.webp"));
            tokio::fs::write(&path, &image.0).await.unwrap();
        }
        panic!("Images generated in /tmp/images. Please verify manually.");
    }
}
