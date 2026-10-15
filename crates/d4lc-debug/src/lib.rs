//! Debug output helpers for traces and image exports.

use d4lc_core::{CaptureFrame, GrayImage, MatchCandidate};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use serde_json::json;
use std::fs;
use std::path::Path;

pub fn write_capture_png(path: &Path, frame: &CaptureFrame) -> Result<(), String> {
    write_file(path, &encode_png_rgba_from_bgra(frame)?)
}

/// Write a grayscale image to a PNG file, creating parent directories as needed.
pub fn write_gray_png(path: &Path, img: &GrayImage) -> Result<(), String> {
    write_file(path, &encode_gray_png(img)?)
}

/// Encode a grayscale image as an 8-bit grayscale PNG.
pub fn encode_gray_png(img: &GrayImage) -> Result<Vec<u8>, String> {
    let expected = usize::try_from(u64::from(img.width) * u64::from(img.height))
        .map_err(|error| error.to_string())?;
    if img.width == 0 || img.height == 0 {
        return Err("cannot encode an empty GrayImage as PNG".to_string());
    }
    if img.pixels.len() != expected {
        return Err(format!(
            "GrayImage pixel buffer has {} bytes but dimensions require {expected}",
            img.pixels.len()
        ));
    }
    encode_png(&img.pixels, img.width, img.height, ExtendedColorType::L8)
}

/// Encode a BGRA capture frame as an RGBA PNG.
pub fn encode_png_rgba_from_bgra(frame: &CaptureFrame) -> Result<Vec<u8>, String> {
    if frame.width == 0 || frame.height == 0 {
        return Err("cannot encode an empty frame as PNG".to_string());
    }
    let width = usize::try_from(frame.width).map_err(|error| error.to_string())?;
    let height = usize::try_from(frame.height).map_err(|error| error.to_string())?;
    let row_len = width * 4;
    if frame.stride < row_len {
        return Err(format!(
            "frame stride {} is smaller than required stride {row_len}",
            frame.stride
        ));
    }
    if frame.pixels_bgra.len() < frame.stride * height {
        return Err(format!(
            "frame buffer has {} bytes but dimensions require {}",
            frame.pixels_bgra.len(),
            frame.stride * height
        ));
    }
    let mut rgba = Vec::with_capacity(row_len * height);
    for row in frame.pixels_bgra.chunks(frame.stride).take(height) {
        for &[b, g, r, a] in row[..row_len].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    encode_png(&rgba, frame.width, frame.height, ExtendedColorType::Rgba8)
}

fn encode_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    color: ExtendedColorType,
) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(pixels, width, height, color)
        .map_err(|error| format!("failed to encode PNG: {error}"))?;
    Ok(png)
}

/// Serialize match candidates to a JSON trace file: the haystack
/// dimensions, every candidate's score and rect, and the best (lowest
/// score) match.
pub fn write_match_trace_json(
    path: &Path,
    haystack_width: u32,
    haystack_height: u32,
    candidates: &[MatchCandidate],
) -> Result<(), String> {
    let candidate = |candidate: &MatchCandidate| {
        json!({
            "template_name": candidate.template_name,
            "score": candidate.score,
            "rect": candidate.rect,
        })
    };
    let best = candidates
        .iter()
        .min_by(|left, right| left.score.total_cmp(&right.score));
    let trace = json!({
        "haystack": { "width": haystack_width, "height": haystack_height },
        "candidates": candidates.iter().map(candidate).collect::<Vec<_>>(),
        "best_match": best.map(candidate),
    });
    write_debug_text(
        path,
        &serde_json::to_string_pretty(&trace).map_err(|error| error.to_string())?,
    )
}

pub fn write_debug_text(path: &Path, contents: &str) -> Result<(), String> {
    write_file(path, contents.as_bytes())
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_bgra_frame_as_an_rgba_png() {
        let frame = CaptureFrame {
            width: 1,
            height: 1,
            stride: 4,
            pixels_bgra: vec![0, 0, 255, 255],
        };

        let png = encode_png_rgba_from_bgra(&frame).unwrap_or_else(|error| panic!("{error}"));
        let decoded = image::load_from_memory(&png)
            .unwrap_or_else(|error| panic!("{error}"))
            .to_rgba8();
        assert_eq!(decoded.as_raw(), &vec![255, 0, 0, 255]);
    }

    #[test]
    fn rejects_stride_smaller_than_width() {
        let frame = CaptureFrame {
            width: 2,
            height: 1,
            stride: 4,
            pixels_bgra: vec![0; 8],
        };

        assert!(encode_png_rgba_from_bgra(&frame).is_err());
    }
}
