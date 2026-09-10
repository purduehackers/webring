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
fn fallback_image(key: &impl Hash) -> WebpScreenshotData {
    const WIDTH: u64 = super::WIDTH as u64;
    const HEIGHT: u64 = super::HEIGHT as u64;
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
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let cell_x = x / CELL;
            let cell_y = y / CELL;
            // Quantize the gradient to whole dither cells so no transition cuts
            // through a cell and creates half-pixels.
            let vertical = cell_y as f32 / (HEIGHT / CELL - 1) as f32;
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
        Encoder::from_rgb(&pixels, WIDTH as u32, HEIGHT as u32)
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
pub struct FallbackImageGenerator;

impl Screenshotter for FallbackImageGenerator {
    fn take_screenshot(
        &self,
        site: Intern<Uri>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<WebpScreenshotData>> + Send + Sync + 'static>>
    {
        Box::pin(std::future::ready(Ok(fallback_image(&*site))))
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use axum::http::Uri;
    use pretty_assertions::assert_ne;
    use sarlacc::Intern;

    use crate::site_previews::capture::Screenshotter;

    use super::FallbackImageGenerator;

    #[tokio::test]
    async fn placeholder_images_are_unique() {
        let uri1 = Intern::new(Uri::from_static("https://kasad.com"));
        let uri2 = Intern::new(Uri::from_static("https://amberzeng.com"));
        assert_ne!(
            FallbackImageGenerator
                .take_screenshot(uri1)
                .await
                .unwrap()
                .0,
            FallbackImageGenerator
                .take_screenshot(uri2)
                .await
                .unwrap()
                .0
        );
    }

    #[tokio::test]
    #[ignore = "this test generates images in /tmp/images for manual inspection to guarantee the colors differ"]
    async fn dummy() {
        for domain in [
            "kasad.com",
            "ericswpark.com",
            "rayhanadev.com",
            "kimjammer.com",
            "neels.page",
        ] {
            let uri = Intern::new(Uri::from_str(&format!("https://{domain}")).unwrap());
            let image = FallbackImageGenerator.take_screenshot(uri).await.unwrap();
            let path = PathBuf::from(format!("/tmp/images/{domain}.webp"));
            tokio::fs::write(&path, &image.0).await.unwrap();
        }
        panic!("Images generated in /tmp/images. Please verify manually.");
    }
}
