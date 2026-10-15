//! Tooltip and marker template loading and the region configuration they
//! feed.

use std::path::Path;

use d4lc_assets::{AssetRepository, TemplateAsset, TemplateKind};
use d4lc_core::GrayImage;
use d4lc_ocr::TooltipOcrRegionConfig;
use d4lc_vision::threshold_binary_inv;
use image::ImageReader;

#[derive(Debug, Clone)]
pub struct MarkerTemplate {
    pub name: String,
    pub kind: TemplateKind,
    pub image: GrayImage,
}

#[derive(Debug, Clone)]
pub(crate) struct TooltipTemplate {
    pub(crate) name: String,
    pub(crate) image: GrayImage,
}

// ---------------------------------------------------------------------------
// Tooltip template loading
// ---------------------------------------------------------------------------

pub(crate) fn load_selected_tooltip_templates(
    repo: &AssetRepository,
    config_root: &Path,
    assets: &[TemplateAsset],
    preset: &str,
    language: &str,
    settings: &d4lc_config::AppSettings,
) -> Result<Vec<TooltipTemplate>, String> {
    // The controller config is config-root relative (`Config/Controllers.json`),
    // not repository relative: a session whose preset templates resolve from the
    // fallback game-data root still reads the user's configured images.
    let configured_controller_images = d4lc_config::load_controller_config(config_root)?;
    let selected_names = if settings.controller_mode {
        repo.active_controller_image_names(preset, language, &configured_controller_images)?
    } else {
        repo.controller_image_names(preset, language)?
            .into_iter()
            .filter(|name| name.starts_with("tooltip_kb_"))
            .collect::<Vec<_>>()
    };

    let mut templates = Vec::new();
    for asset in assets {
        if asset.kind != TemplateKind::Tooltip {
            continue;
        }
        if !selected_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&asset.file_name))
        {
            continue;
        }
        let image = load_template_gray_image(&repo.root().join(&asset.relative_path))?;
        templates.push(TooltipTemplate {
            name: asset.file_name.clone(),
            image,
        });
    }
    templates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(templates)
}

// ---------------------------------------------------------------------------
// Marker template loading
// ---------------------------------------------------------------------------

pub(crate) fn load_marker_templates(
    repo: &AssetRepository,
    assets: &[TemplateAsset],
    threshold_min: u8,
    threshold_max: u8,
) -> Result<Vec<MarkerTemplate>, String> {
    let mut templates = Vec::new();
    for asset in assets {
        if !matches!(
            asset.kind,
            TemplateKind::AffixMarker
                | TemplateKind::AspectMarker
                | TemplateKind::SocketMarker
                | TemplateKind::SplitterMarker
        ) {
            continue;
        }
        let path = repo.root().join(&asset.relative_path);
        let gray = load_template_gray_image(&path)?;
        templates.push(MarkerTemplate {
            name: asset.file_name.clone(),
            kind: asset.kind,
            image: threshold_binary_inv(&gray, threshold_min, threshold_max),
        });
    }
    templates.sort_by(|left, right| {
        left.kind
            .as_str()
            .cmp(right.kind.as_str())
            .then(left.name.cmp(&right.name))
    });
    Ok(templates)
}

/// Add the d4lc supplement marker templates the game-data root ships for
/// `preset` (`d4lc_assets::SUPPLEMENT_PRESETS_DIR`). A template whose name the
/// loaded preset already carries is skipped, so an upstream preset file of
/// the same name always wins.
pub(crate) fn add_supplement_marker_templates(
    templates: &mut Vec<MarkerTemplate>,
    game_data_repo: &AssetRepository,
    preset: &str,
    threshold_min: u8,
    threshold_max: u8,
) -> Result<(), String> {
    let supplement: Vec<TemplateAsset> = game_data_repo
        .supplement_templates_for_preset(preset)?
        .into_iter()
        .filter(|asset| {
            !templates
                .iter()
                .any(|template| template.name.eq_ignore_ascii_case(&asset.file_name))
        })
        .collect();
    if supplement.is_empty() {
        return Ok(());
    }
    templates.extend(load_marker_templates(
        game_data_repo,
        &supplement,
        threshold_min,
        threshold_max,
    )?);
    templates.sort_by(|left, right| {
        left.kind
            .as_str()
            .cmp(right.kind.as_str())
            .then(left.name.cmp(&right.name))
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Region configuration
// ---------------------------------------------------------------------------

pub(crate) fn tooltip_region_config(
    marker_templates: &[MarkerTemplate],
    tooltip_width: u32,
    tooltip_height: u32,
    settings: &d4lc_config::AppSettings,
) -> Result<TooltipOcrRegionConfig, String> {
    Ok(TooltipOcrRegionConfig {
        tooltip_width,
        tooltip_height,
        // Upstream reads the stored `Settings.TooltipMaxHeight`, seeded from
        // the preset's TooltipHeightType on a preset change
        // (ScreenProcessHandler.cs:695-696, SettingsViewModel.cs:269-271).
        tooltip_max_height: settings.tooltip_max_height,
        // The four area offsets come from the stored settings the same way
        // (`SettingsD4.cs:9-12`, seeded from the preset's config,
        // `SettingsViewModel.cs:258-261`); the preset's own config values
        // are only the seed source.
        affix_area_height_offset_top: settings.affix_area_height_offset_top,
        affix_area_height_offset_bottom: settings.affix_area_height_offset_bottom,
        affix_aspect_area_width_offset: settings.affix_aspect_area_width_offset,
        aspect_area_height_offset_top: settings.aspect_area_height_offset_top,
        affix_marker_width: first_marker_width(marker_templates, TemplateKind::AffixMarker),
        aspect_marker_width: first_marker_width(marker_templates, TemplateKind::AspectMarker),
        type_area_offset_left: settings.type_area_offset_left,
        type_area_offset_right: settings.type_area_offset_right,
        aspect_area_max_height: settings.aspect_area_max_height,
    })
}

pub(crate) fn first_marker_width(templates: &[MarkerTemplate], kind: TemplateKind) -> u32 {
    templates
        .iter()
        .find(|template| template.kind == kind)
        .map(|template| template.image.width)
        .unwrap_or(0)
}

/// Largest accepted template side. Every shipped and downloadable preset
/// PNG is at most 180 × 180.
pub const TEMPLATE_IMAGE_MAX_SIDE: u32 = 4096;

/// Decode a template PNG into an upstream grayscale image. Preset images
/// come from downloaded archives, so the decoder runs under explicit
/// limits instead of the `image` default (no size limit, 512 MiB
/// allocations): at most [`TEMPLATE_IMAGE_MAX_SIDE`] per side and the
/// allocation of one such RGBA8 image. Oversized headers fail before the
/// pixel buffer is allocated.
pub fn load_template_gray_image(path: &Path) -> Result<GrayImage, String> {
    let mut reader = ImageReader::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TEMPLATE_IMAGE_MAX_SIDE);
    limits.max_image_height = Some(TEMPLATE_IMAGE_MAX_SIDE);
    limits.max_alloc = Some(u64::from(TEMPLATE_IMAGE_MAX_SIDE).pow(2) * 4);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("failed to decode {}: {error}", path.display()))?
        .to_rgba8();
    // Upstream `new Image<Gray, byte>(fileName)` (ScreenProcessHandler.cs:248)
    // converts with OpenCV's BT.601 weights; `image::to_luma8` uses Rec.709
    // and moves coloured marker pixels across the binary threshold.
    let (width, height) = image.dimensions();
    Ok(d4lc_vision::gray_from_rgba8(width, height, image.as_raw()))
}

#[cfg(test)]
mod tests;
