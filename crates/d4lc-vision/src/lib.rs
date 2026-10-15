//! d4lc-vision — grayscale conversion, threshold helpers, and template matching.
//!
//! Upstream: D4Companion.Services/ScreenProcessHandler.cs

pub mod matcher;
pub use matcher::{
    match_template_sqdiff_normed, match_template_sqdiff_normed_below,
    match_template_sqdiff_normed_masked, match_templates, match_templates_below,
    match_templates_below_parallel_timed, scan_threads_for_host, MatchScratch, ParallelMatchScan,
    TemplateMatchResult,
};

use d4lc_core::{CaptureFrame, GrayImage};

/// One 8-bit RGB pixel as OpenCV's `cvtColor(…GRAY)` computes it.
///
/// Upstream converts every image with Emgu `Convert<Gray, byte>()` (the
/// screen and tooltip captures) or loads it with `new Image<Gray, byte>(file)`
/// (the preset marker and tooltip templates, ScreenProcessHandler.cs:248);
/// both end in OpenCV's 8-bit RGB→GRAY conversion, the BT.601 weights in
/// 14-bit fixed point with rounding:
/// `Y = (4899·R + 9617·G + 1868·B + 2^13) >> 14`.
/// A truncating or differently weighted conversion moves pixels across the
/// binary threshold and changes the template scores.
#[inline]
pub fn gray_from_rgb(r: u8, g: u8, b: u8) -> u8 {
    let y = (4899 * u32::from(r) + 9617 * u32::from(g) + 1868 * u32::from(b) + (1 << 13)) >> 14;
    // The weights sum to 2^14, so `y` never exceeds 255.
    y.min(255) as u8
}

/// An RGBA8 pixel buffer (alpha ignored, like OpenCV's colour load) as an
/// upstream grayscale image ([`gray_from_rgb`]).
pub fn gray_from_rgba8(width: u32, height: u32, rgba: &[u8]) -> GrayImage {
    let (pixels, _) = rgba.as_chunks::<4>();
    GrayImage {
        width,
        height,
        pixels: pixels
            .iter()
            .map(|&[r, g, b, _]| gray_from_rgb(r, g, b))
            .collect(),
    }
}

/// Convert a BGRA capture frame to an 8-bit grayscale image.
///
/// Upstream: ScreenProcessHandler.cs
///   `currentScreen.Convert<Gray, byte>()`
///   `_currentScreenTooltip.Convert<Gray, byte>()`
///
/// Uses OpenCV's conversion ([`gray_from_rgb`]).
pub fn to_grayscale(frame: &CaptureFrame) -> GrayImage {
    let mut gray = GrayImage {
        width: 0,
        height: 0,
        pixels: Vec::new(),
    };
    to_grayscale_into(frame, &mut gray);
    gray
}

/// [`to_grayscale`] into an existing image, reusing its pixel allocation.
///
/// Every pixel of `out` is rewritten with the same formula, so the result is
/// identical to a fresh [`to_grayscale`] whatever `out` held before.
pub fn to_grayscale_into(frame: &CaptureFrame, out: &mut GrayImage) {
    let width = frame.width as usize;
    let height = frame.height as usize;
    out.width = frame.width;
    out.height = frame.height;
    out.pixels.resize(width * height, 0);
    if width == 0 {
        return;
    }
    for (y, gray_row) in out.pixels.chunks_exact_mut(width).enumerate() {
        let row_start = y * frame.stride;
        let bgra_row = &frame.pixels_bgra[row_start..row_start + width * 4];
        let (pixels, _) = bgra_row.as_chunks::<4>();
        for (gray, &[b, g, r, _]) in gray_row.iter_mut().zip(pixels) {
            *gray = gray_from_rgb(r, g, b);
        }
    }
}

/// Apply OpenCV THRESH_BINARY_INV to a grayscale image.
///
/// Upstream: ScreenProcessHandler.cs
///   `image.ThresholdBinaryInv(new Gray(threshold), new Gray(max_val))`
///   `_currentScreenTooltip.Convert<Gray,byte>().ThresholdBinaryInv(...)`
///
/// Rule: pixel > threshold → 0, else → max_val.
/// Default upstream values from SettingsD4.cs: threshold=70, max_val=255.
pub fn threshold_binary_inv(img: &GrayImage, threshold: u8, max_val: u8) -> GrayImage {
    let pixels = img
        .pixels
        .iter()
        .map(|&p| if p > threshold { 0 } else { max_val })
        .collect();
    GrayImage {
        width: img.width,
        height: img.height,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_core::CaptureFrame;

    // 4×4 BGRA fixture (B,G,R,A per pixel, row-major).
    // Grayscale expected values (BT.601 integer, truncating):
    //   Row 0: 0, 50, 100, 150
    //   Row 1: 200, 255, 38, 75
    //   Row 2: 14, 70, 71, 69
    //   Row 3: 135, 85, 80, 20
    // ThresholdBinaryInv(70,255) expected:
    //   Row 0: 255,255,0,0  Row 1: 0,0,255,0
    //   Row 2: 255,255,0,255  Row 3: 0,0,0,255
    fn fixture_bgra() -> CaptureFrame {
        #[rustfmt::skip]
        let pixels: Vec<u8> = vec![
            0,0,0,255,       50,50,50,255,     100,100,100,255, 150,150,150,255,
            200,200,200,255, 255,255,255,255,  0,0,128,255,     0,128,0,255,
            128,0,0,255,     70,70,70,255,     71,71,71,255,    69,69,69,255,
            30,200,50,255,   120,30,180,255,   80,80,80,255,    0,0,70,255,
        ];
        CaptureFrame {
            width: 4,
            height: 4,
            stride: 16,
            pixels_bgra: pixels,
        }
    }

    #[test]
    fn to_grayscale_uniform_pixels_pass_through() {
        // Uniform RGB: Y = R = G = B exactly.
        let pixels: Vec<u8> = vec![
            0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255, 100, 100, 100, 255,
        ];
        let frame = CaptureFrame {
            width: 4,
            height: 1,
            stride: 16,
            pixels_bgra: pixels,
        };
        let gray = to_grayscale(&frame);
        assert_eq!(gray.pixels, vec![0, 128, 255, 100]);
    }

    #[test]
    fn to_grayscale_pure_channels() {
        // OpenCV RGB→GRAY, (w*128 + 8192) >> 14:
        // Pure R=128 → (4899*128 + 8192) >> 14 = 38
        // Pure G=128 → (9617*128 + 8192) >> 14 = 75
        // Pure B=128 → (1868*128 + 8192) >> 14 = 15 (rounded, not truncated)
        let pixels: Vec<u8> = vec![
            0, 0, 128, 255, // R=128
            0, 128, 0, 255, // G=128
            128, 0, 0, 255, // B=128
        ];
        let frame = CaptureFrame {
            width: 3,
            height: 1,
            stride: 12,
            pixels_bgra: pixels,
        };
        let gray = to_grayscale(&frame);
        assert_eq!(gray.pixels, vec![38, 75, 15]);
    }

    /// The conversion rounds like OpenCV instead of truncating: a pixel
    /// just above a gray level lands on the threshold's far side otherwise.
    #[test]
    fn gray_from_rgb_rounds_like_opencv() {
        // 9617*129 = 1240593 -> (1240593 + 8192) >> 14 = 76 (76.2);
        // the truncating 587*129/1000 = 75.7 gave 75.
        assert_eq!(gray_from_rgb(0, 129, 0), 76);
        // Rec.709 (`image::to_luma8`) gives 61 for this red-pink pixel, dark
        // at threshold 70; OpenCV gives 78, light.
        assert_eq!(gray_from_rgb(200, 20, 60), 78);
        assert_eq!(gray_from_rgb(255, 255, 255), 255);
        assert_eq!(gray_from_rgb(0, 0, 0), 0);
        let image = gray_from_rgba8(2, 1, &[0, 129, 0, 255, 0, 0, 128, 0]);
        assert_eq!(image.pixels, vec![76, 15]);
    }

    #[test]
    fn to_grayscale_4x4_fixture() {
        let gray = to_grayscale(&fixture_bgra());
        assert_eq!(gray.width, 4);
        assert_eq!(gray.height, 4);
        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            0, 50, 100, 150,
            200, 255, 38, 75,
            15, 70, 71, 69,
            136, 85, 80, 21,
        ];
        assert_eq!(gray.pixels, expected);
    }

    #[test]
    fn threshold_binary_inv_at_boundary() {
        // pixel == threshold (70) → not strictly greater → max_val
        let img = GrayImage {
            width: 3,
            height: 1,
            pixels: vec![69, 70, 71],
        };
        let result = threshold_binary_inv(&img, 70, 255);
        assert_eq!(result.pixels, vec![255, 255, 0]);
    }

    #[test]
    fn threshold_binary_inv_4x4_fixture() {
        let gray = to_grayscale(&fixture_bgra());
        let result = threshold_binary_inv(&gray, 70, 255);
        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            255, 255, 0,   0,
            0,   0,   255, 0,
            255, 255, 0,   255,
            0,   0,   0,   255,
        ];
        assert_eq!(result.pixels, expected);
    }

    #[test]
    fn threshold_binary_inv_custom_max_val() {
        let img = GrayImage {
            width: 2,
            height: 1,
            pixels: vec![10, 200],
        };
        let result = threshold_binary_inv(&img, 100, 128);
        assert_eq!(result.pixels, vec![128, 0]);
    }
}
