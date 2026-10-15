//! Live in-memory debug bundle of one accepted tooltip analysis.
//!
//! When `Settings.IsDebugInfoEnabled` is set, upstream
//! `ScreenProcessHandler` draws one debug bitmap per processing stage and
//! publishes it over the messenger:
//!
//! - the tooltip crop (`FindTooltips`, `ScreenProcessHandler.cs:605-620`);
//! - the item-type/power area (`FindItemTypesPower`,
//!   `ScreenProcessHandler.cs:691-735`);
//! - the affix locations (`FindItemAffixLocations`,
//!   `ScreenProcessHandler.cs:786-802`);
//! - the affix areas (`FindItemAffixAreas`,
//!   `ScreenProcessHandler.cs:967-979`);
//! - the aspect location/area (`FindItemAspectLocations` /
//!   `FindItemAspectAreas`, `ScreenProcessHandler.cs:1239-1252`,
//!   `1382-1389`);
//! - the socket locations (`FindItemSocketLocations`,
//!   `ScreenProcessHandler.cs:1537-1547`);
//! - the splitter locations (`FindItemSplitterLocations`,
//!   `ScreenProcessHandler.cs:1666-1676`).
//!
//! This module rebuilds those stages from the OCR execution traces the port
//! already has ([`TooltipOcrExecution`]: the crop trace, the detected marker
//! plan and the per-segment recognition results). It never re-detects and
//! never re-OCRs, and it never touches the disk — the screenshot/export
//! bundle stays the only writer.
//!
//! Everything here is built once per OCR job on the OCR worker thread (off
//! the frame hot path) and only when the debug bit is on; with the bit off no
//! stage is allocated at all.

use std::sync::Arc;

use d4lc_core::{CaptureFrame, Rect};
use d4lc_ocr::{OcrImage, OcrImagePixels, OcrSegmentKind};

use crate::ocr_parity::{segment_ocr_image, TooltipOcrExecution};

/// One immutable RGBA8 stage bitmap: tightly packed, top-down rows,
/// `width * height * 4` bytes. `Arc`-shared so the UI can hand it to the
/// renderer without copying the pixels (`slint::SharedPixelBuffer::clone_from_slice`).
#[derive(Debug, Clone)]
pub struct DebugImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl DebugImage {
    fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba: Arc::from(rgba),
        }
    }
}

/// Equality is shared-buffer identity: two stage bitmaps are equal when they
/// are the same allocation. A freshly built bundle therefore never compares
/// equal to an older one, so the accepted-analysis equality check
/// (`RuntimeCoordinator`/`LiveState`) recognizes a replaced bundle without
/// comparing megabytes of pixels.
impl PartialEq for DebugImage {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && Arc::ptr_eq(&self.rgba, &other.rgba)
    }
}

impl Eq for DebugImage {}

/// One exact OCR segment image: the pixels the segment handed to the OCR
/// backend (upstream has no separate bitmap for this; the port keeps the
/// recognized image observable per segment, `export_tooltip_ocr_chain`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugSegmentImage {
    pub index: usize,
    pub kind: &'static str,
    pub rect: Rect,
    pub image: DebugImage,
}

/// One recognized OCR text line and where it sits in the tooltip crop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugTextLine {
    pub segment_index: usize,
    pub segment_kind: &'static str,
    pub text: String,
    pub rect: Rect,
}

/// The stage bitmaps and text lines of one accepted tooltip analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct TooltipDebugBundle {
    /// The final tooltip crop the stages were drawn on (upstream
    /// `_currentScreenTooltip`, `ScreenProcessHandler.cs:635`).
    pub tooltip_crop: DebugImage,
    /// The item-type/power area crop (`ScreenProcessHandler.cs:702-713`).
    /// `None` when no upper-tooltip segment was recognized.
    pub item_type_area: Option<DebugImage>,
    /// The tooltip crop with the detected affix locations outlined
    /// (`ScreenProcessHandler.cs:786-800`).
    pub affix_locations: DebugImage,
    /// The tooltip crop with the planned affix OCR areas outlined
    /// (`ScreenProcessHandler.cs:947-979`).
    pub affix_areas: DebugImage,
    /// The tooltip crop with the aspect location/area outlined
    /// (`ScreenProcessHandler.cs:1239-1252`, `1382-1389`).
    pub aspect_area: DebugImage,
    /// The tooltip crop with the detected socket locations outlined
    /// (`ScreenProcessHandler.cs:1537-1547`).
    pub socket_locations: DebugImage,
    /// The tooltip crop with the detected splitter locations outlined
    /// (`ScreenProcessHandler.cs:1666-1676`).
    pub splitter_locations: DebugImage,
    /// The exact image of every OCR segment, in plan order.
    pub segments: Vec<DebugSegmentImage>,
    /// Every recognized text line with its segment provenance and geometry.
    pub lines: Vec<DebugTextLine>,
}

/// Upstream draws a stage marker as a 2-pixel outline in
/// `new MCvScalar(0, 0, 255)` — red in BGRA
/// (`CvInvoke.Rectangle`, e.g. `ScreenProcessHandler.cs:792`). The bundle
/// stores RGBA, so the outline is `(255, 0, 0, 255)`.
const MARKER_THICKNESS: i64 = 2;

impl TooltipDebugBundle {
    /// Build the bundle from one completed OCR execution.
    ///
    /// Callers gate this on `Settings.IsDebugInfoEnabled`; with the bit off
    /// this is never called and no stage is allocated.
    pub fn from_execution(execution: &TooltipOcrExecution) -> Self {
        let frame = &execution.ocr_frame;
        let (width, height) = (frame.width, frame.height);
        let base = bgra_to_rgba(frame);

        let affix_locations: Vec<Rect> = execution
            .marker_plan
            .affix_markers
            .iter()
            .map(|marker| marker.rect)
            .collect();
        let socket_locations = execution.marker_plan.socket_markers.clone();
        let splitter_locations: Vec<Rect> = execution
            .marker_plan
            .splitter_markers
            .iter()
            .map(|marker| marker.rect)
            .collect();
        // The planned affix OCR areas are the areas upstream's
        // `FindItemAffixAreas` builds and outlines (`SPH.cs:947-979`); a
        // rune/sigil item resolves the same areas to those kinds.
        let affix_area_rects: Vec<Rect> = execution
            .results
            .iter()
            .filter(|result| {
                matches!(
                    result.plan.kind,
                    OcrSegmentKind::Affix | OcrSegmentKind::Rune | OcrSegmentKind::Sigil
                )
            })
            .map(|result| result.plan.rect)
            .collect();
        let aspect_rects: Vec<Rect> = execution
            .results
            .iter()
            .filter(|result| {
                matches!(
                    result.plan.kind,
                    OcrSegmentKind::Aspect | OcrSegmentKind::UniqueAspect
                )
            })
            .map(|result| result.plan.rect)
            .chain(
                execution
                    .marker_plan
                    .aspect_marker
                    .iter()
                    .map(|marker| marker.rect),
            )
            .collect();

        let item_type_area = execution
            .results
            .iter()
            .find(|result| result.plan.kind == OcrSegmentKind::UpperTooltip)
            .and_then(|result| crop_rgba(&base, width, height, result.plan.rect));

        let segments = execution
            .results
            .iter()
            .filter_map(|result| {
                let image =
                    segment_ocr_image(frame, result.plan.rect, result.ocr_input.variant).ok()?;
                Some(DebugSegmentImage {
                    index: result.plan.index,
                    kind: result.plan.kind.as_str(),
                    rect: result.plan.rect,
                    image: ocr_image_to_rgba(&image),
                })
            })
            .collect();
        let lines = execution
            .results
            .iter()
            .flat_map(|result| {
                let kind = result.plan.kind.as_str();
                result.text.lines.iter().map(move |line| DebugTextLine {
                    segment_index: result.plan.index,
                    segment_kind: kind,
                    text: line.text.clone(),
                    rect: line.rect,
                })
            })
            .collect();

        Self {
            tooltip_crop: DebugImage::new(width, height, base.clone()),
            item_type_area,
            affix_locations: stage(&base, width, height, &affix_locations),
            affix_areas: stage(&base, width, height, &affix_area_rects),
            aspect_area: stage(&base, width, height, &aspect_rects),
            socket_locations: stage(&base, width, height, &socket_locations),
            splitter_locations: stage(&base, width, height, &splitter_locations),
            segments,
            lines,
        }
    }
}

/// A copy of `base` with every `rect` outlined like upstream draws it.
fn stage(base: &[u8], width: u32, height: u32, rects: &[Rect]) -> DebugImage {
    let mut rgba = base.to_vec();
    for rect in rects {
        draw_rect_outline(&mut rgba, width, height, *rect);
    }
    DebugImage::new(width, height, rgba)
}

/// The whole BGRA capture as tightly packed RGBA8.
fn bgra_to_rgba(frame: &CaptureFrame) -> Vec<u8> {
    let row_bytes = usize::try_from(frame.width).unwrap_or(0).saturating_mul(4);
    let height = usize::try_from(frame.height).unwrap_or(0);
    let mut out = vec![0_u8; row_bytes.saturating_mul(height)];
    for row in 0..height {
        let src_start = row.saturating_mul(frame.stride);
        let Some(src) = frame
            .pixels_bgra
            .get(src_start..src_start.saturating_add(row_bytes))
        else {
            break;
        };
        let dst_start = row.saturating_mul(row_bytes);
        let Some(dst) = out.get_mut(dst_start..dst_start.saturating_add(row_bytes)) else {
            break;
        };
        for (source, target) in src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<4>().0)
        {
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            target[3] = 255;
        }
    }
    out
}

/// The RGBA8 image of one OCR segment input.
fn ocr_image_to_rgba(image: &OcrImage) -> DebugImage {
    match &image.pixels {
        OcrImagePixels::Bgra(frame) => {
            DebugImage::new(frame.width, frame.height, bgra_to_rgba(frame))
        }
        OcrImagePixels::Gray(gray) => {
            let mut rgba = Vec::with_capacity(gray.pixels.len().saturating_mul(4));
            for value in &gray.pixels {
                rgba.extend_from_slice(&[*value, *value, *value, 255]);
            }
            DebugImage::new(gray.width, gray.height, rgba)
        }
    }
}

/// The `rect` of `src` (RGBA8, `src_width` wide) as its own image, clamped to
/// the source. `None` for an empty or out-of-bounds rect.
fn crop_rgba(src: &[u8], src_width: u32, src_height: u32, rect: Rect) -> Option<DebugImage> {
    let source_w = i64::from(src_width);
    let source_h = i64::from(src_height);
    let x0 = i64::from(rect.x).clamp(0, source_w);
    let y0 = i64::from(rect.y).clamp(0, source_h);
    let x1 = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, source_w);
    let y1 = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, source_h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let width = u32::try_from(x1 - x0).ok()?;
    let height = u32::try_from(y1 - y0).ok()?;
    let row_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    let source_row_bytes = usize::try_from(src_width).ok()?.checked_mul(4)?;
    let mut out = Vec::with_capacity(row_bytes.checked_mul(usize::try_from(height).ok()?)?);
    for y in y0..y1 {
        let row_start = usize::try_from(y).ok()?.checked_mul(source_row_bytes)?;
        let start = row_start.checked_add(usize::try_from(x0).ok()?.checked_mul(4)?)?;
        out.extend_from_slice(src.get(start..start.checked_add(row_bytes)?)?);
    }
    Some(DebugImage::new(width, height, out))
}

/// Draw upstream's 2-pixel red outline of `rect` into `rgba`.
fn draw_rect_outline(rgba: &mut [u8], width: u32, height: u32, rect: Rect) {
    let image_w = i64::from(width);
    let image_h = i64::from(height);
    if image_w <= 0 || image_h <= 0 {
        return;
    }
    let x0 = i64::from(rect.x).clamp(0, image_w);
    let y0 = i64::from(rect.y).clamp(0, image_h);
    let x1 = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, image_w);
    let y1 = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, image_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let on_border = x - x0 < MARKER_THICKNESS
                || x1 - 1 - x < MARKER_THICKNESS
                || y - y0 < MARKER_THICKNESS
                || y1 - 1 - y < MARKER_THICKNESS;
            if !on_border {
                continue;
            }
            let Ok(x) = u32::try_from(x) else { continue };
            let Ok(y) = u32::try_from(y) else { continue };
            if let Some(pixel) = pixel_mut(rgba, width, x, y) {
                pixel[0] = 255;
                pixel[1] = 0;
                pixel[2] = 0;
                pixel[3] = 255;
            }
        }
    }
}

fn pixel_mut(rgba: &mut [u8], width: u32, x: u32, y: u32) -> Option<&mut [u8; 4]> {
    let index = usize::try_from(y)
        .ok()?
        .checked_mul(usize::try_from(width).ok()?)?
        .checked_add(usize::try_from(x).ok()?)?
        .checked_mul(4)?;
    let end = index.checked_add(4)?;
    <&mut [u8; 4]>::try_from(rgba.get_mut(index..end)?).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::runtime_ocr_support::test_execution;
    use super::*;

    #[test]
    fn bgra_is_converted_to_rgba_row_by_row() {
        // Two rows with a wider stride: the padding must not leak into RGBA.
        let frame = CaptureFrame {
            width: 2,
            height: 2,
            stride: 12,
            pixels_bgra: vec![
                1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, // row 0 + 4 padding
                7, 8, 9, 255, 10, 11, 12, 255, 9, 9, 9, 9, // row 1 + 4 padding
            ],
        };
        let rgba = bgra_to_rgba(&frame);
        assert_eq!(rgba.len(), 16);
        assert_eq!(&rgba[0..4], &[3, 2, 1, 255]);
        assert_eq!(&rgba[4..8], &[6, 5, 4, 255]);
        assert_eq!(&rgba[8..12], &[9, 8, 7, 255]);
        assert_eq!(&rgba[12..16], &[12, 11, 10, 255]);
    }

    #[test]
    fn outline_is_two_pixels_and_red() {
        let mut rgba = vec![0_u8; 8 * 8 * 4];
        draw_rect_outline(
            &mut rgba,
            8,
            8,
            Rect {
                x: 2,
                y: 2,
                width: 6,
                height: 6,
            },
        );
        // The border (2 px inward) is red, the hole stays transparent black.
        assert_eq!(&rgba[pixel(8, 2, 2)..pixel(8, 2, 2) + 4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[pixel(8, 3, 3)..pixel(8, 3, 3) + 4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[pixel(8, 4, 4)..pixel(8, 4, 4) + 4], &[0, 0, 0, 0]);
        assert_eq!(&rgba[pixel(8, 6, 6)..pixel(8, 6, 6) + 4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[pixel(8, 0, 0)..pixel(8, 0, 0) + 4], &[0, 0, 0, 0]);
    }

    fn pixel(width: usize, x: usize, y: usize) -> usize {
        (y * width + x) * 4
    }

    #[test]
    fn crop_clamps_to_the_source() {
        let src = vec![7_u8; 4 * 4 * 4];
        let cropped = crop_rgba(
            &src,
            4,
            4,
            Rect {
                x: 2,
                y: 2,
                width: 10,
                height: 10,
            },
        )
        .unwrap_or_else(|| panic!("crop must succeed"));
        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert_eq!(cropped.rgba.len(), 2 * 2 * 4);
        assert!(crop_rgba(
            &src,
            4,
            4,
            Rect {
                x: 9,
                y: 9,
                width: 1,
                height: 1,
            }
        )
        .is_none());
    }

    /// The bundle carries every upstream stage from the execution's own
    /// traces: the crop, the marker stages, the item-type area, the exact
    /// segment images and the recognized text lines.
    #[test]
    fn bundle_carries_the_upstream_stages_from_the_execution() {
        let execution = test_execution::fixture();
        let bundle = TooltipDebugBundle::from_execution(&execution);

        assert_eq!(
            (bundle.tooltip_crop.width, bundle.tooltip_crop.height),
            (execution.ocr_frame.width, execution.ocr_frame.height)
        );
        assert!(
            bundle.tooltip_crop.rgba.len() as u64
                == u64::from(bundle.tooltip_crop.width) * u64::from(bundle.tooltip_crop.height) * 4,
            "the crop is a tightly packed RGBA buffer"
        );

        // The affix-location stage outlines the detected marker in red.
        let marker = execution.marker_plan.affix_markers[0].rect;
        let index = pixel(
            usize::try_from(bundle.affix_locations.width).unwrap_or(0),
            usize::try_from(marker.x).unwrap_or(0),
            usize::try_from(marker.y).unwrap_or(0),
        );
        assert_eq!(
            &bundle.affix_locations.rgba[index..index + 4],
            &[255, 0, 0, 255],
            "the detected affix location is outlined like upstream SPH.cs:792"
        );

        let item_type = bundle
            .item_type_area
            .as_ref()
            .unwrap_or_else(|| panic!("upper-tooltip area must be cropped"));
        assert_eq!((item_type.width, item_type.height), (320, 40));

        assert_eq!(bundle.segments.len(), execution.results.len());
        assert_eq!(bundle.segments[0].kind, "upper_tooltip");
        assert_eq!(bundle.segments[0].index, 0);
        assert_eq!(bundle.lines.len(), 2);
        assert_eq!(bundle.lines[0].text, "Magic Rune");
        assert_eq!(bundle.lines[0].segment_index, 0);
    }

    /// A freshly built bundle never compares equal to another one: the UI
    /// rebuild of an accepted analysis always replaces the shown stages.
    #[test]
    fn distinct_bundles_are_never_equal() {
        let execution = test_execution::fixture();
        let first = TooltipDebugBundle::from_execution(&execution);
        let second = TooltipDebugBundle::from_execution(&execution);
        assert_ne!(first, second);
        assert_eq!(first.clone(), first);
    }
}

/// A hand-built [`TooltipOcrExecution`] for the bundle tests. Kept in a tiny
/// support module so the production paths do not carry test-only imports.
#[cfg(test)]
mod runtime_ocr_support {
    pub mod test_execution {
        //! A minimal execution fixture: one upper-tooltip segment and one
        //! affix segment, with one detected affix marker.

        use crate::ocr_parity::{
            ocr_input_trace, DetectedMarkerPlan, SegmentResult, SplitterDetectionTrace,
            TooltipCropTrace, TooltipOcrExecution, TooltipOcrPreprocessVariant,
            UPSTREAM_SPLITTER_DETECTION_RULE, UPSTREAM_TOOLTIP_CROP_RULE,
        };
        use d4lc_core::{CaptureFrame, OcrBoundingBoxSource, OcrLine, OcrText, Rect};
        use d4lc_ocr::{
            OcrBackendKind, OcrImage, OcrMarker, OcrPageSegMode, OcrProfile, OcrSegmentKind,
            OcrSegmentPlan,
        };

        pub fn fixture() -> TooltipOcrExecution {
            let frame = white_frame(375, 120);
            let upper = segment(0, OcrSegmentKind::UpperTooltip, "Magic Rune", 0);
            let affix = segment(1, OcrSegmentKind::Affix, "+10 Strength", 60);
            TooltipOcrExecution {
                profile: OcrProfile {
                    backend: OcrBackendKind::Auto,
                    language: "enUS".to_string(),
                    tessdata_path: None,
                },
                preset: "1080p_SMF".to_string(),
                ocr_frame: frame,
                preprocess_variant: TooltipOcrPreprocessVariant::RawBgra,
                crop_trace: TooltipCropTrace {
                    planned_tooltip_roi: whole(),
                    search_roi: whole(),
                    detected_tooltip_rect: None,
                    final_tooltip_crop_rect: whole(),
                    crop_source: "detector_final_crop",
                    upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
                    original_input_size: (375, 120),
                    final_tooltip_size: (375, 120),
                },
                marker_plan: DetectedMarkerPlan {
                    affix_markers: vec![OcrMarker {
                        kind: OcrSegmentKind::Affix,
                        name: "dot-affixes_normal_1".to_string(),
                        rect: Rect {
                            x: 8,
                            y: 60,
                            width: 20,
                            height: 20,
                        },
                    }],
                    aspect_markers: Vec::new(),
                    aspect_marker: None,
                    socket_markers: Vec::new(),
                    splitter_markers: Vec::new(),
                    splitter_detection: SplitterDetectionTrace {
                        roi: whole(),
                        threshold: 0.05,
                        template_names: Vec::new(),
                        template_results: Vec::new(),
                        accepted_matches: Vec::new(),
                        splitters_before_cleanup: Vec::new(),
                        splitters_after_cleanup: Vec::new(),
                        top_splitter_cleanup_applied: false,
                        removed_overlapping_splitters: Vec::new(),
                        upstream_rule: UPSTREAM_SPLITTER_DETECTION_RULE,
                    },
                    splitter_detection_ms: 0,
                    aspect_area_trace: None,
                    is_unique_item: false,
                    has_tooltip_top_splitter: false,
                },
                results: vec![upper, affix],
            }
        }

        fn whole() -> Rect {
            Rect {
                x: 0,
                y: 0,
                width: 375,
                height: 120,
            }
        }

        fn white_frame(width: u32, height: u32) -> CaptureFrame {
            let stride = usize::try_from(width).unwrap_or(0) * 4;
            CaptureFrame {
                width,
                height,
                stride,
                pixels_bgra: vec![255_u8; stride * usize::try_from(height).unwrap_or(0)],
            }
        }

        fn segment(index: usize, kind: OcrSegmentKind, text: &str, y: i32) -> SegmentResult {
            let rect = Rect {
                x: 0,
                y,
                width: 320,
                height: 40,
            };
            let image = OcrImage::new(white_frame(rect.width, rect.height));
            let input = ocr_input_trace(&image, TooltipOcrPreprocessVariant::RawBgra);
            SegmentResult {
                plan: OcrSegmentPlan {
                    index,
                    kind,
                    source: "tooltip_filter".to_string(),
                    rect,
                    source_marker_rect: None,
                    boundary_source: "tooltip_bottom".to_string(),
                    boundary_rect: None,
                    upstream_rule: "test".to_string(),
                    marker_name: None,
                    marker_index: None,
                    page_seg_mode: OcrPageSegMode::Default,
                    language: "enUS".to_string(),
                    tooltip_width: 0,
                    clamp_reason: None,
                },
                text: OcrText {
                    raw_text: text.to_string(),
                    lines: vec![OcrLine {
                        text: text.to_string(),
                        rect: Rect {
                            x: 4,
                            y,
                            width: 200,
                            height: 14,
                        },
                        words: Vec::new(),
                    }],
                    bounding_box_source: OcrBoundingBoxSource::Unavailable,
                },
                tsv: None,
                ocr_input: input,
                converted_text: text.to_string(),
                conversion_method: "",
                conversion_branch: "",
                right_aligned_line_filtering_applied: true,
                type_area_offset_left: 0,
                ocr_ms: 0,
                conversion_ms: 0,
            }
        }
    }
}
