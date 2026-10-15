use crate::{OcrPageSegMode, OcrSegmentKind};
use d4lc_core::Rect;

/// OCR marker detected inside a tooltip crop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrMarker {
    pub kind: OcrSegmentKind,
    pub name: String,
    pub rect: Rect,
}

/// OCR region derived from upstream marker geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrRegion {
    pub kind: OcrSegmentKind,
    pub name: String,
    pub source_marker_rect: Option<Rect>,
    pub rect: Rect,
    pub boundary_source: String,
    pub boundary_rect: Option<Rect>,
}

/// Upstream item-affix marker classification from `ScreenProcessHandler.FindItemAffixAreas`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AffixType {
    #[default]
    Normal,
    Greater,
    Tempered,
    Rune,
}

impl AffixType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Greater => "greater",
            Self::Tempered => "tempered",
            Self::Rune => "rune",
        }
    }
}

/// OCR region plus upstream affix type metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrAffixRegion {
    pub region: OcrRegion,
    pub affix_type: AffixType,
    pub marker_index: usize,
}

/// Planned OCR segment emitted by the upstream-parity region planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrSegmentPlan {
    pub index: usize,
    pub kind: OcrSegmentKind,
    pub source: String,
    pub rect: Rect,
    pub source_marker_rect: Option<Rect>,
    pub boundary_source: String,
    pub boundary_rect: Option<Rect>,
    pub upstream_rule: String,
    pub marker_name: Option<String>,
    pub marker_index: Option<usize>,
    pub page_seg_mode: OcrPageSegMode,
    pub language: String,
    /// The full tooltip width before any per-segment offset (e.g., type_area_offset_left).
    /// Used in debug traces to reconstruct the original rectangle before offset was applied.
    pub tooltip_width: u32,
    /// If a per-segment offset was clamped, records the reason string.
    /// `None` means no clamping was needed.
    pub clamp_reason: Option<String>,
}

impl OcrSegmentPlan {
    pub fn is_full_tooltip(&self, tooltip_width: u32, tooltip_height: u32) -> bool {
        self.rect
            == (Rect {
                x: 0,
                y: 0,
                width: tooltip_width,
                height: tooltip_height,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrSegmentPlanInput {
    pub config: TooltipOcrRegionConfig,
    pub affix_markers: Vec<OcrMarker>,
    pub aspect_marker: Option<OcrMarker>,
    pub socket_markers: Vec<Rect>,
    pub splitter_markers: Vec<OcrMarker>,
    pub language: String,
    pub socket_detection_enabled: bool,
    pub tempered_detection_enabled: bool,
    /// Upstream `Settings.IsAspectDetectionEnabled` (`ScreenProcessHandler.cs:471`).
    pub aspect_detection_enabled: bool,
    /// Upstream `Settings.IsUniqueDetectionEnabled` (`ScreenProcessHandler.cs:476`).
    pub unique_detection_enabled: bool,
    /// Upstream `Settings.IsRuneDetectionEnabled` (`ScreenProcessHandler.cs:419`).
    pub rune_detection_enabled: bool,
    pub item_type: Option<String>,
    pub is_unique_item: bool,
    pub has_tooltip_top_splitter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooltipOcrRegionConfig {
    pub tooltip_width: u32,
    pub tooltip_height: u32,
    pub tooltip_max_height: u32,
    pub affix_area_height_offset_top: i32,
    pub affix_area_height_offset_bottom: i32,
    pub affix_aspect_area_width_offset: i32,
    pub aspect_area_height_offset_top: i32,
    pub affix_marker_width: u32,
    pub aspect_marker_width: u32,
    pub type_area_offset_left: u32,
    /// Upstream `Settings.TypeAreaOffsetRight` (v5.4.0), default 5.
    pub type_area_offset_right: u32,
    /// Upstream `Settings.AspectAreaMaxHeight` (v5.4.0), default 300.
    pub aspect_area_max_height: u32,
}

/// Mirrors `FindItemTypesPower`: OCR the section above the first splitter when present.
pub fn upper_tooltip_region(
    config: TooltipOcrRegionConfig,
    splitters: &[Rect],
) -> Option<OcrRegion> {
    if config.tooltip_width == 0 || config.tooltip_height == 0 {
        return None;
    }

    // Apply type_area_offset_left clamping: if offset >= tooltip_width, fall back to 0 to avoid empty/invalid regions.
    let effective_offset: i32 = if config.type_area_offset_left >= config.tooltip_width {
        0
    } else {
        config.type_area_offset_left as i32
    };

    // Upstream FindItemTypesPower: width = (Width - offsetLeft - offsetRight) <= 0
    // ? Width - offsetLeft : Width - offsetLeft - offsetRight.
    let tooltip_width = i64::from(config.tooltip_width);
    let width_with_right =
        tooltip_width - i64::from(effective_offset) - i64::from(config.type_area_offset_right);
    let fallback_width = tooltip_width - i64::from(effective_offset);
    let region_width = if width_with_right <= 0 {
        fallback_width
    } else {
        width_with_right
    };
    let region_width = u32::try_from(region_width.max(0)).unwrap_or(u32::MAX);

    let top_splitter = splitters.iter().min_by_key(|rect| rect.y).copied();
    let (rect, boundary_source, boundary_rect) = if let Some(first_splitter) = top_splitter {
        let max_height = i32::try_from(config.tooltip_max_height).ok()?;
        let start_y = first_splitter.y.saturating_sub(max_height).max(0);
        let height = first_splitter.y.min(max_height).max(0);
        if height == 0 {
            return None;
        }
        (
            Rect {
                x: effective_offset,
                y: start_y,
                width: region_width,
                height: u32::try_from(height).ok()?,
            },
            "first_splitter",
            top_splitter,
        )
    } else {
        (
            Rect {
                x: effective_offset,
                y: 0,
                width: region_width,
                height: config.tooltip_height,
            },
            "tooltip_full_height",
            None,
        )
    };

    Some(OcrRegion {
        kind: OcrSegmentKind::UpperTooltip,
        name: "upper_tooltip".to_string(),
        source_marker_rect: top_splitter,
        rect,
        boundary_source: boundary_source.to_string(),
        boundary_rect,
    })
}

/// Mirrors upstream affix/aspect area construction from detected marker rectangles.
pub fn marker_defined_regions(
    config: TooltipOcrRegionConfig,
    affix_markers: &[OcrMarker],
    aspect_marker: Option<&OcrMarker>,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
) -> Vec<OcrRegion> {
    let mut regions = Vec::new();
    regions.extend(
        affix_regions(
            config,
            affix_markers,
            aspect_marker,
            socket_markers,
            splitter_markers,
        )
        .into_iter()
        .map(|affix| affix.region),
    );

    if let Some(marker) = aspect_marker {
        if let Some(region) = aspect_region(
            config,
            affix_markers,
            marker,
            socket_markers,
            splitter_markers,
            false,
        ) {
            regions.push(region);
        }
    }

    regions.sort_by_key(|left| left.rect.y);
    regions
}

pub fn affix_regions(
    config: TooltipOcrRegionConfig,
    affix_markers: &[OcrMarker],
    aspect_marker: Option<&OcrMarker>,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
) -> Vec<OcrAffixRegion> {
    let affix_split_points = split_points_for_affixes(
        affix_markers,
        aspect_marker,
        socket_markers,
        splitter_markers,
    );
    let mut indexed_markers = affix_markers.iter().enumerate().collect::<Vec<_>>();
    indexed_markers.sort_by_key(|left| left.1.rect.y);

    let mut regions = Vec::new();
    for (marker_index, marker) in indexed_markers {
        if let Some(region) = marker_text_region(
            config,
            marker,
            &affix_split_points,
            OcrSegmentKind::Affix,
            config.affix_marker_width,
            config.affix_area_height_offset_top,
        ) {
            regions.push(OcrAffixRegion {
                region,
                affix_type: affix_type_from_marker_name(&marker.name),
                marker_index,
            });
        }
    }
    regions
}

pub fn aspect_region(
    config: TooltipOcrRegionConfig,
    affix_markers: &[OcrMarker],
    aspect_marker: &OcrMarker,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
    is_unique_item: bool,
) -> Option<OcrRegion> {
    let aspect_split_points = split_points_for_aspect(
        affix_markers,
        aspect_marker,
        socket_markers,
        splitter_markers,
    );
    let mut region = marker_text_region(
        config,
        aspect_marker,
        &aspect_split_points,
        if is_unique_item {
            OcrSegmentKind::UniqueAspect
        } else {
            OcrSegmentKind::Aspect
        },
        config.aspect_marker_width,
        config.aspect_area_height_offset_top,
    )?;

    // Upstream FindItemAspectAreas (v5.4.0): height = min(AspectAreaMaxHeight, height).
    region.rect.height = region.rect.height.min(config.aspect_area_max_height);
    if region.rect.height == 0 {
        return None;
    }

    Some(region)
}

pub fn plan_ocr_segments(input: OcrSegmentPlanInput) -> Vec<OcrSegmentPlan> {
    let splitter_rects = input
        .splitter_markers
        .iter()
        .map(|marker| marker.rect)
        .collect::<Vec<_>>();
    let mut plans = Vec::new();

    if input.has_tooltip_top_splitter {
        if let Some(region) = upper_tooltip_region(input.config, &splitter_rects) {
            let marker_index = input
                .splitter_markers
                .iter()
                .enumerate()
                .min_by_key(|(_, marker)| marker.rect.y)
                .map(|(index, _)| index);
            plans.push(OcrSegmentPlan {
                index: plans.len(),
                kind: OcrSegmentKind::UpperTooltip,
                source: "tooltip_filter".to_string(),
                rect: region.rect,
                source_marker_rect: region.source_marker_rect,
                boundary_source: region.boundary_source,
                boundary_rect: region.boundary_rect,
                upstream_rule: "ScreenProcessHandler.FindItemTypesPower: startY=max(0, first_splitter_y-TooltipMaxHeight); height=min(first_splitter_y, TooltipMaxHeight); OcrHandler.ConvertToTextUpperTooltipSection uses PageSegMode.SparseText".to_string(),
                marker_name: marker_index.and_then(|index| input.splitter_markers.get(index).map(|marker| marker.name.clone())),
                marker_index,
                page_seg_mode: OcrPageSegMode::SparseText,
                language: input.language.clone(),
                tooltip_width: input.config.tooltip_width,
                clamp_reason: if input.config.type_area_offset_left >= input.config.tooltip_width {
                    Some(format!("type_area_offset_left {} clamped to 0 (>= tooltip_width {})", input.config.type_area_offset_left, input.config.tooltip_width))
                } else if input.config.type_area_offset_left > 0 {
                    Some(format!("type_area_offset_left {} applied", input.config.type_area_offset_left))
                } else {
                    None
                },
            });
        }
    }

    let mut affix_areas = affix_regions(
        input.config,
        &input.affix_markers,
        input.aspect_marker.as_ref(),
        &input.socket_markers,
        &splitter_rects,
    );
    if !input.tempered_detection_enabled {
        affix_areas.retain(|area| area.affix_type != AffixType::Tempered);
    }
    if input
        .item_type
        .as_deref()
        .map(is_upstream_rune_item_type)
        .unwrap_or(false)
    {
        affix_areas.retain(|area| area.affix_type == AffixType::Rune);
    }
    // Upstream "Skip disabled item types - Config" (ScreenProcessHandler.cs:418-421):
    // a rune tooltip with rune detection off clears ItemAffixLocations.
    if !input.rune_detection_enabled
        && input
            .item_type
            .as_deref()
            .map(is_upstream_rune_item_type)
            .unwrap_or(false)
    {
        affix_areas.clear();
    }
    // Upstream "Skip disabled item types - Not implemented": charm and
    // horadric-seal tooltips clear ItemAffixLocations, so no affix OCR runs.
    if input
        .item_type
        .as_deref()
        .map(is_upstream_affix_areas_cleared_item_type)
        .unwrap_or(false)
    {
        affix_areas.clear();
    }
    // Upstream "Skip ignored item types - Seasonal": seasonal sigil tooltips
    // clear ItemAffixLocations, so no affix OCR runs.
    if input
        .item_type
        .as_deref()
        .map(is_upstream_seasonal_sigil_item_type)
        .unwrap_or(false)
    {
        affix_areas.clear();
    }
    affix_areas.sort_by_key(|left| left.region.rect.y);

    // Upstream FindItemAffix checks the sigil item type before the marker
    // type: itemType == Sigil converts every affix area with
    // OcrHandler.ConvertToSigil (sigils have no dedicated marker template).
    let is_sigil_item_type = input
        .item_type
        .as_deref()
        .map(is_upstream_sigil_item_type)
        .unwrap_or(false);

    for (area_index, area) in affix_areas.into_iter().enumerate() {
        let region = area.region;
        plans.push(OcrSegmentPlan {
            index: plans.len(),
            kind: if is_sigil_item_type {
                OcrSegmentKind::Sigil
            } else if area.affix_type == AffixType::Rune {
                OcrSegmentKind::Rune
            } else {
                OcrSegmentKind::Affix
            },
            source: "tooltip_filter".to_string(),
            rect: region.rect,
            source_marker_rect: region.source_marker_rect,
            boundary_source: region.boundary_source,
            boundary_rect: region.boundary_rect,
            upstream_rule: "ScreenProcessHandler.FindItemAffixAreas/FindItemAffixes: marker x plus marker_width*1.2; y minus AffixAreaHeightOffsetTop; next split y minus AffixAreaHeightOffsetBottom; OcrHandler.ConvertToText applies no footer/control-text filter".to_string(),
            marker_name: Some(region.name),
            marker_index: Some(area_index),
            page_seg_mode: OcrPageSegMode::Default,
            language: input.language.clone(),
            tooltip_width: 0,
            clamp_reason: None,
        });
    }

    // Upstream (ScreenProcessHandler.cs:471-484): a non-unique tooltip needs
    // IsAspectDetectionEnabled and a unique/mythic one IsUniqueDetectionEnabled,
    // otherwise no aspect OCR runs. The aspect location above still bounds the
    // affix area, because upstream reads it before this gate.
    let aspect_detection_enabled = if input.is_unique_item {
        input.unique_detection_enabled
    } else {
        input.aspect_detection_enabled
    };
    if let Some(aspect_marker) = input
        .aspect_marker
        .as_ref()
        .filter(|_| aspect_detection_enabled)
    {
        if let Some(region) = aspect_region(
            input.config,
            &input.affix_markers,
            aspect_marker,
            &input.socket_markers,
            &splitter_rects,
            input.is_unique_item,
        ) {
            plans.push(OcrSegmentPlan {
                index: plans.len(),
                kind: region.kind,
                source: "tooltip_filter".to_string(),
                rect: region.rect,
                source_marker_rect: region.source_marker_rect,
                boundary_source: region.boundary_source,
                boundary_rect: region.boundary_rect,
                upstream_rule: "ScreenProcessHandler.FindItemAspectAreas/FindItemAspects/FindItemUniqueAspects: marker x plus marker_width*1.2; y minus AspectAreaHeightOffsetTop; next split y minus AffixAreaHeightOffsetBottom, otherwise tooltip image bottom; OcrHandler.ConvertToText applies no footer/control-text filter".to_string(),
                marker_name: Some(region.name),
                marker_index: Some(0),
                page_seg_mode: OcrPageSegMode::Default,
                language: input.language,
                tooltip_width: 0,
                clamp_reason: None,
            });
        }
    }

    plans
}

pub fn affix_type_from_marker_name(name: &str) -> AffixType {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("dot-affixes_greater") {
        AffixType::Greater
    } else if lower.starts_with("dot-affixes_temper") {
        AffixType::Tempered
    } else if lower.starts_with("dot-affixes_rune") {
        AffixType::Rune
    } else {
        AffixType::Normal
    }
}

fn is_upstream_rune_item_type(item_type: &str) -> bool {
    item_type.eq_ignore_ascii_case("rune")
}

/// Upstream `ItemTypeConstants.Sigil`. `ScreenProcessHandler.FindItemAffix`
/// selects `OcrHandler.ConvertToSigil` for every affix area when the tooltip
/// item type is sigil; the item type itself comes from
/// `OcrHandler.ConvertToItemType` on the upper tooltip section (no item
/// power, last-lines fuzzy match).
fn is_upstream_sigil_item_type(item_type: &str) -> bool {
    item_type.eq_ignore_ascii_case("sigil")
}

/// Upstream `ItemTypeConstants.Charm` / `ItemTypeConstants.HoradricSeal`.
/// Affix areas are cleared for these item types because affix detection is
/// "Not implemented" for them upstream (ScreenProcessHandler.ProcessTooltip).
fn is_upstream_affix_areas_cleared_item_type(item_type: &str) -> bool {
    item_type.eq_ignore_ascii_case("charm") || item_type.eq_ignore_ascii_case("horadricseal")
}

/// Upstream `ItemTypeConstants.WitcherSigil` /
/// `ItemTypeConstants.DungeonEscalation` / `ItemTypeConstants.BloodiedLair`.
/// Seasonal sigil tooltips clear ItemAffixLocations in the "Skip ignored item
/// types - Seasonal" block of `ScreenProcessHandler.FindItemAffix`, so their
/// affix areas are never OCR'd or sigil-classified.
fn is_upstream_seasonal_sigil_item_type(item_type: &str) -> bool {
    item_type.eq_ignore_ascii_case("witchersigil")
        || item_type.eq_ignore_ascii_case("dungeonescalation")
        || item_type.eq_ignore_ascii_case("bloodiedlair")
}

fn split_points_for_affixes(
    affix_markers: &[OcrMarker],
    aspect_marker: Option<&OcrMarker>,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
) -> Vec<SplitPoint> {
    // Upstream `FindItemAffixAreas` offsets socket markers by the aspect marker y-position when
    // present, otherwise by the last affix y-position.
    split_points(
        affix_markers,
        aspect_marker,
        socket_markers,
        splitter_markers,
        socket_offset_base_y(affix_markers, aspect_marker),
    )
}

fn split_points_for_aspect(
    affix_markers: &[OcrMarker],
    aspect_marker: &OcrMarker,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
) -> Vec<SplitPoint> {
    // Upstream `FindItemAspectAreas` falls back to the last affix y-position when no aspect
    // marker exists. This helper is only called with an aspect marker, so it uses aspect.y.
    split_points(
        affix_markers,
        Some(aspect_marker),
        socket_markers,
        splitter_markers,
        aspect_marker.rect.y,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SplitPoint {
    rect: Rect,
    source: SplitPointSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitPointSource {
    Affix,
    Aspect,
    Socket,
    Splitter,
}

impl SplitPointSource {
    const fn as_boundary_source(self) -> &'static str {
        match self {
            Self::Affix => "affix_marker",
            Self::Aspect => "aspect_marker",
            Self::Socket => "socket_marker",
            Self::Splitter => "splitter_marker",
        }
    }
}

const UPSTREAM_SOCKET_SPLIT_POINT_OFFSET_Y: i32 = 4;
const UPSTREAM_SOCKET_SPLIT_POINT_HEIGHT_REDUCTION: u32 = 2;

fn socket_offset_base_y(affix_markers: &[OcrMarker], aspect_marker: Option<&OcrMarker>) -> i32 {
    aspect_marker
        .map(|marker| marker.rect.y)
        .or_else(|| affix_markers.last().map(|marker| marker.rect.y))
        .unwrap_or(0)
}

fn split_points(
    affix_markers: &[OcrMarker],
    aspect_marker: Option<&OcrMarker>,
    socket_markers: &[Rect],
    splitter_markers: &[Rect],
    socket_y_offset: i32,
) -> Vec<SplitPoint> {
    let mut split_points = Vec::new();
    split_points.extend(affix_markers.iter().map(|marker| SplitPoint {
        rect: marker.rect,
        source: SplitPointSource::Affix,
    }));
    if let Some(marker) = aspect_marker {
        split_points.push(SplitPoint {
            rect: marker.rect,
            source: SplitPointSource::Aspect,
        });
    }
    if let Some(socket) = socket_markers.first().copied() {
        split_points.push(SplitPoint {
            rect: Rect {
                y: socket
                    .y
                    .saturating_add(socket_y_offset)
                    .saturating_add(UPSTREAM_SOCKET_SPLIT_POINT_OFFSET_Y),
                height: socket
                    .height
                    .saturating_sub(UPSTREAM_SOCKET_SPLIT_POINT_HEIGHT_REDUCTION),
                ..socket
            },
            source: SplitPointSource::Socket,
        });
    }
    split_points.extend(splitter_markers.iter().copied().map(|rect| SplitPoint {
        rect,
        source: SplitPointSource::Splitter,
    }));
    split_points.sort_by_key(|left| left.rect.y);
    split_points
}

fn marker_text_region(
    config: TooltipOcrRegionConfig,
    marker: &OcrMarker,
    split_points: &[SplitPoint],
    kind: OcrSegmentKind,
    marker_width: u32,
    top_offset: i32,
) -> Option<OcrRegion> {
    let offset_marker = ((f64::from(marker_width)) * 1.2) as i32;
    let boundary = split_points
        .iter()
        .find(|split| split.rect.y > marker.rect.y)
        .copied();
    let (next_y, boundary_source, boundary_rect) = if let Some(split) = boundary {
        (
            split
                .rect
                .y
                .saturating_sub(config.affix_area_height_offset_bottom),
            split.source.as_boundary_source().to_string(),
            Some(split.rect),
        )
    } else {
        (
            i32::try_from(config.tooltip_height).unwrap_or(i32::MAX),
            "tooltip_bottom".to_string(),
            None,
        )
    };
    let y = marker.rect.y.saturating_sub(top_offset);
    let x = marker.rect.x.saturating_add(offset_marker);
    let right = i32::try_from(config.tooltip_width)
        .ok()?
        .saturating_sub(config.affix_aspect_area_width_offset);
    let width = right.saturating_sub(x);
    let height = next_y.saturating_sub(y);
    if width <= 0 || height <= 0 {
        return None;
    }

    Some(OcrRegion {
        kind,
        name: marker.name.clone(),
        source_marker_rect: Some(marker.rect),
        rect: Rect {
            x,
            y,
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
        },
        boundary_source,
        boundary_rect,
    })
}

#[cfg(test)]
mod tests;
