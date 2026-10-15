//! Antialiased marker shapes both backends composite the same way.
//!
//! Upstream draws its markers through Direct2D, which antialiases every
//! edge. The X11 core `poly_fill_arc`/`poly_arc` pair and a per-pixel
//! inside test both produce stair-stepped circle edges (and the core arc
//! stroke leaves a nub where it starts and ends), so the circle is
//! rasterized here with coverage and handed out as a premultiplied ARGB32
//! image: X11 composites it OVER the window through RENDER, Wayland blends
//! it into its canvas with [`crate::text::blend_over`].

use d4lc_core::Rect;

/// Samples per pixel axis; 4x4 gives 17 coverage levels.
const SUPERSAMPLE: u32 = 4;

/// Width of upstream's circle outline (`OutlineFillCircle(..., stroke: 2)`).
const OUTLINE_WIDTH: f64 = 2.0;

/// The opaque black outline, premultiplied.
const OUTLINE_ARGB: u32 = 0xFF00_0000;

/// A premultiplied ARGB32 image placed at an absolute position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeImage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// `width * height` premultiplied ARGB32 pixels, row-major.
    pub pixels: Vec<u32>,
}

impl ShapeImage {
    /// The little-endian byte payload (`put_image` ZPixmap at 32 bits per
    /// pixel, `wl_shm` ARGB8888).
    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .collect()
    }
}

/// Upstream `Graphics.OutlineFillCircle`: `fill` (premultiplied ARGB32)
/// inside, a 2px black outline along the inside of the circle's bounding
/// rect (an ellipse for a non-square rect), with antialiased edges. `None`
/// for an empty rect.
pub fn outline_fill_circle(rect: Rect, fill: u32) -> Option<ShapeImage> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let rx = f64::from(rect.width) / 2.0;
    let ry = f64::from(rect.height) / 2.0;
    let inner_rx = rx - OUTLINE_WIDTH;
    let inner_ry = ry - OUTLINE_WIDTH;
    let inside = |dx: f64, dy: f64, rx: f64, ry: f64| {
        rx > 0.0 && ry > 0.0 && (dx / rx).powi(2) + (dy / ry).powi(2) <= 1.0
    };

    let samples = SUPERSAMPLE * SUPERSAMPLE;
    let step = 1.0 / f64::from(SUPERSAMPLE);
    let mut pixels = Vec::with_capacity((rect.width as usize) * (rect.height as usize));
    for row in 0..rect.height {
        for column in 0..rect.width {
            let mut outer = 0u32;
            let mut inner = 0u32;
            for sample_y in 0..SUPERSAMPLE {
                let dy = f64::from(row) + (f64::from(sample_y) + 0.5) * step - ry;
                for sample_x in 0..SUPERSAMPLE {
                    let dx = f64::from(column) + (f64::from(sample_x) + 0.5) * step - rx;
                    if inside(dx, dy, rx, ry) {
                        outer += 1;
                        if inside(dx, dy, inner_rx, inner_ry) {
                            inner += 1;
                        }
                    }
                }
            }
            pixels.push(mix(fill, inner, outer - inner, samples));
        }
    }
    Some(ShapeImage {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        pixels,
    })
}

/// The pixel `fill_samples` of `fill` plus `outline_samples` of the black
/// outline make, out of `samples`; the rest of the pixel stays transparent.
fn mix(fill: u32, fill_samples: u32, outline_samples: u32, samples: u32) -> u32 {
    let channel = |shift: u32| {
        let fill_part = ((fill >> shift) & 0xFF) * fill_samples;
        let outline_part = ((OUTLINE_ARGB >> shift) & 0xFF) * outline_samples;
        ((fill_part + outline_part + samples / 2) / samples).min(255) << shift
    };
    channel(24) | channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(size: u32) -> ShapeImage {
        let rect = Rect {
            x: 10,
            y: 20,
            width: size,
            height: size,
        };
        outline_fill_circle(rect, 0xFF00_8000).unwrap_or_else(|| panic!("circle"))
    }

    fn pixel(image: &ShapeImage, x: u32, y: u32) -> u32 {
        image.pixels[(y * image.width + x) as usize]
    }

    #[test]
    fn the_centre_is_the_fill_and_the_corners_stay_transparent() {
        let image = circle(20);
        assert_eq!(
            (image.x, image.y, image.width, image.height),
            (10, 20, 20, 20)
        );
        assert_eq!(pixel(&image, 10, 10), 0xFF00_8000);
        assert_eq!(pixel(&image, 0, 0), 0);
        assert_eq!(pixel(&image, 19, 19), 0);
    }

    #[test]
    fn the_outline_is_black_along_the_inside_of_the_rect() {
        let image = circle(20);
        // One pixel into the left edge on the centre row: solid outline.
        assert_eq!(pixel(&image, 0, 10), 0xFF00_0000);
        assert_eq!(pixel(&image, 1, 10), 0xFF00_0000);
        // Past the 2px outline the fill starts.
        assert_eq!(pixel(&image, 3, 10), 0xFF00_8000);
    }

    #[test]
    fn diagonal_edges_are_antialiased_and_symmetric() {
        let image = circle(20);
        let partial = image
            .pixels
            .iter()
            .filter(|pixel| {
                let alpha = *pixel >> 24;
                alpha > 0 && alpha < 255
            })
            .count();
        assert!(partial > 0, "the circle edge must carry partial coverage");
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(pixel(&image, x, y), pixel(&image, 19 - x, y));
                assert_eq!(pixel(&image, x, y), pixel(&image, x, 19 - y));
            }
        }
    }

    #[test]
    fn an_empty_rect_has_no_image() {
        assert!(outline_fill_circle(
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 8
            },
            0xFF00_8000
        )
        .is_none());
    }
}
