use crate::{
    crop_frame, CaptureBackend, CaptureMethod, CaptureReference, CaptureRequest, CaptureResult,
    CaptureScopeResult, CaptureSource, OverlayWindowState, RoiOrigin, ScaleFactor,
};
use d4lc_core::{CaptureFrame, Rect};

/// Explicit non-production frame source for deterministic tests and harnesses.
///
/// This source never participates in window discovery and is only used when a
/// caller wires it in directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticCaptureFrameSource {
    reference: CaptureReference,
    frame: CaptureFrame,
}

impl SyntheticCaptureFrameSource {
    pub fn new(reference: CaptureReference, frame: CaptureFrame) -> Result<Self, String> {
        if reference.geometry.width == 0 || reference.geometry.height == 0 {
            return Err("synthetic capture reference geometry must be non-empty".to_string());
        }

        if frame.width != reference.geometry.width || frame.height != reference.geometry.height {
            return Err(format!(
                "synthetic capture frame dimensions {}x{} do not match reference geometry {}x{}",
                frame.width, frame.height, reference.geometry.width, reference.geometry.height
            ));
        }

        let expected_row_bytes = usize::try_from(frame.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| {
                "synthetic capture frame width overflowed row-size calculation".to_string()
            })?;

        if frame.stride < expected_row_bytes {
            return Err(format!(
                "synthetic capture frame stride {} is smaller than required row width {}",
                frame.stride, expected_row_bytes
            ));
        }

        let required_len = usize::try_from(frame.height)
            .ok()
            .and_then(|height| frame.stride.checked_mul(height))
            .ok_or_else(|| {
                "synthetic capture frame height overflowed buffer-size calculation".to_string()
            })?;

        if frame.pixels_bgra.len() < required_len {
            return Err(format!(
                "synthetic capture frame buffer has {} bytes but requires at least {}",
                frame.pixels_bgra.len(),
                required_len
            ));
        }

        Ok(Self { reference, frame })
    }

    pub fn manual_region(frame: CaptureFrame) -> Result<Self, String> {
        let geometry = Rect {
            x: 0,
            y: 0,
            width: frame.width,
            height: frame.height,
        };
        Self::new(
            CaptureReference {
                source: CaptureSource::ManualRegion(geometry),
                geometry,
            },
            frame,
        )
    }

    pub fn reference(&self) -> &CaptureReference {
        &self.reference
    }

    /// Move the synthetic window on screen: the stand-in for a game window
    /// the window manager moved. The frame keeps its size, so only a
    /// same-size geometry is accepted; returns whether the origin changed.
    pub fn move_to(&mut self, geometry: Rect) -> Result<bool, String> {
        if geometry.width != self.frame.width || geometry.height != self.frame.height {
            return Err(format!(
                "synthetic capture frame is {}x{}; it cannot follow a {}x{} window",
                self.frame.width, self.frame.height, geometry.width, geometry.height
            ));
        }
        if self.reference.geometry == geometry {
            return Ok(false);
        }
        self.reference.geometry = geometry;
        if let CaptureSource::ManualRegion(region) = &mut self.reference.source {
            *region = geometry;
        }
        Ok(true)
    }

    pub fn capture(&self, request: CaptureRequest) -> Result<CaptureResult, String> {
        let full_rect = Rect {
            x: 0,
            y: 0,
            width: self.frame.width,
            height: self.frame.height,
        };

        let (requested_capture_rect, actual_capture_rect, roi_origin, capture_scope, crop_region) =
            match request {
                CaptureRequest::Full => (
                    full_rect,
                    full_rect,
                    RoiOrigin::FullFrame,
                    CaptureScopeResult::FullFrame,
                    None,
                ),
                CaptureRequest::WindowRelativeRoi(rect) => (
                    rect,
                    rect,
                    RoiOrigin::WindowRelative,
                    CaptureScopeResult::Roi,
                    Some(rect),
                ),
                CaptureRequest::ScreenGlobalRoi(rect) => (
                    rect,
                    rect,
                    RoiOrigin::ScreenGlobal,
                    CaptureScopeResult::Roi,
                    Some(Rect {
                        x: rect.x - self.reference.geometry.x,
                        y: rect.y - self.reference.geometry.y,
                        width: rect.width,
                        height: rect.height,
                    }),
                ),
            };

        let frame = match crop_region {
            Some(region) => crop_frame(&self.frame, region)?,
            None => self.frame.clone(),
        };

        Ok(CaptureResult {
            backend: CaptureBackend::X11Command,
            fallback_reason: Some("synthetic capture frame source".to_string()),
            source: self.reference.source.clone(),
            geometry: self.reference.geometry,
            scale: ScaleFactor::one(),
            requested_capture_rect,
            actual_capture_rect,
            roi_origin,
            capture_scope,
            overlay_window: OverlayWindowState::Ignored,
            capture_method: CaptureMethod::X11Command,
            frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_pixel_frame() -> CaptureFrame {
        CaptureFrame {
            width: 2,
            height: 1,
            stride: 8,
            pixels_bgra: vec![0, 0, 255, 255, 0, 255, 0, 255],
        }
    }

    #[test]
    fn manual_region_full_capture_preserves_frame() {
        let source = SyntheticCaptureFrameSource::manual_region(two_pixel_frame())
            .unwrap_or_else(|error| panic!("synthetic setup failed: {error}"));

        let capture = source
            .capture(CaptureRequest::Full)
            .unwrap_or_else(|error| panic!("synthetic capture failed: {error}"));

        assert_eq!(capture.geometry.width, 2);
        assert_eq!(capture.capture_scope, CaptureScopeResult::FullFrame);
        assert_eq!(
            capture.frame.pixels_bgra,
            vec![0, 0, 255, 255, 0, 255, 0, 255]
        );
        assert_eq!(
            capture.fallback_reason.as_deref(),
            Some("synthetic capture frame source")
        );
    }

    #[test]
    fn window_relative_roi_capture_crops_frame() {
        let source = SyntheticCaptureFrameSource::manual_region(two_pixel_frame())
            .unwrap_or_else(|error| panic!("synthetic setup failed: {error}"));

        let capture = source
            .capture(CaptureRequest::WindowRelativeRoi(Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            }))
            .unwrap_or_else(|error| panic!("synthetic capture failed: {error}"));

        assert_eq!(capture.capture_scope, CaptureScopeResult::Roi);
        assert_eq!(capture.roi_origin, RoiOrigin::WindowRelative);
        assert_eq!(capture.frame.width, 1);
        assert_eq!(capture.frame.pixels_bgra, vec![0, 255, 0, 255]);
    }
}
