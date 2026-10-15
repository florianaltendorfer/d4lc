//! Original data and image asset loading for d4lc.
//!
//! The data in `assets/original` is copied from the MIT-licensed upstream
//! Diablo4Companion repository and stays file based so later parser, detector,
//! OCR, and rule crates can consume the exact upstream artifacts.

pub mod catalog;

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub const ASSET_ROOT: &str = "assets/original";
pub const DATA_DIR: &str = "assets/original/data";
pub const SYSTEM_PRESETS_DIR: &str = "assets/original/systempresets";
pub const ASSET_MANIFEST_PATH: &str = "assets/provenance/asset-manifest.json";

/// Extracted user preset directories, relative to the repository root the
/// [`AssetRepository`] is rooted at.
///
/// Upstream extracts every downloaded community preset zip into
/// `.\Images\` (`ZipFile.ExtractToDirectory`, SystemPresetManager.cs:107-125);
/// the archive carries a single `{preset}/` root directory, so the
/// extracted tree lands at `Images\{preset}\`. The Linux port keeps the
/// same directory naming under the config root
/// (`config_root/Images/{preset}` — d4lc-app
/// `actions::settings::PRESETS_IMAGES_SUBDIR`). Extracted user presets
/// take precedence over the vendored `assets/original/systempresets` tree;
/// the vendored tree stays the
/// default/fallback for every name without an extracted copy.
pub const EXTRACTED_PRESETS_DIR: &str = "Images";

/// Coordinates the readers of the extracted preset trees
/// ([`EXTRACTED_PRESETS_DIR`]`/{preset}`) with their replacement.
///
/// A preset install swaps the live tree in with two renames, so the
/// directory is briefly absent, and [`AssetRepository`] would then resolve
/// the preset to the vendored tree of the same name, or to nothing. Code
/// that reads one preset in several steps holds
/// [`lock_preset_trees_shared`] for the whole read and sees either the old
/// or the new tree. The installer holds [`lock_preset_trees_exclusive`] only
/// around its renames, never while it downloads or extracts.
///
/// The lock is process-wide and not reentrant: a thread takes it at most
/// once at a time.
static PRESET_TREES: RwLock<()> = RwLock::new(());

/// Shared access to the extracted preset trees (see [`PRESET_TREES`]).
pub fn lock_preset_trees_shared() -> RwLockReadGuard<'static, ()> {
    // The guarded data is `()`: a panic while holding the lock leaves no
    // state behind that a poisoned lock would protect.
    PRESET_TREES.read().unwrap_or_else(PoisonError::into_inner)
}

/// Exclusive access to the extracted preset trees, for replacing one (see
/// [`PRESET_TREES`]).
pub fn lock_preset_trees_exclusive() -> RwLockWriteGuard<'static, ()> {
    PRESET_TREES.write().unwrap_or_else(PoisonError::into_inner)
}

/// d4lc-authored marker templates that supplement an upstream system preset,
/// relative to the game-data root (`{dir}/{preset}/*.png`).
///
/// Upstream accepts a tempered affix that became a greater affix as Greater
/// ("Since season 11 a tempered affix can become a greater affix",
/// AffixManager.cs:547-549), but its presets ship no template for that
/// marker: the tempered greater star carries an orange rim the
/// `dot-affixes_greater.png` template does not match within
/// `ThresholdSimilarityAffixLocation`. The supplement adds the missing
/// templates under upstream file-name conventions (`dot-affixes_greater*` →
/// Greater, SPH.cs:961); an upstream preset file of the same name always
/// wins. Provenance per file: `assets/d4lc/README.md`.
pub const SUPPLEMENT_PRESETS_DIR: &str = "assets/d4lc/systempresets";

pub const DATA_CATEGORIES: &[&str] = &[
    "Affixes",
    "Aspects",
    "ItemTypes",
    "ParagonBoards",
    "ParagonGlyphs",
    "Runes",
    "Sigils",
    "Uniques",
];

pub const REQUIRED_LOCALIZED_DATA_CATEGORIES: &[&str] = DATA_CATEGORIES;

pub const LANGUAGES: &[&str] = &[
    "deDE", "enUS", "esES", "esMX", "frFR", "itIT", "jaJP", "koKR", "plPL", "ptBR", "ruRU", "trTR",
    "zhCN", "zhTW",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRepository {
    root: PathBuf,
}

impl AssetRepository {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn asset_root(&self) -> PathBuf {
        self.root.join(ASSET_ROOT)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    pub fn system_presets_dir(&self) -> PathBuf {
        self.root.join(SYSTEM_PRESETS_DIR)
    }

    /// Directory holding extracted user preset trees (upstream `Images\`).
    pub fn extracted_presets_dir(&self) -> PathBuf {
        self.root.join(EXTRACTED_PRESETS_DIR)
    }

    /// Directory holding the d4lc supplement trees ([`SUPPLEMENT_PRESETS_DIR`]).
    pub fn supplement_presets_dir(&self) -> PathBuf {
        self.root.join(SUPPLEMENT_PRESETS_DIR)
    }

    /// The d4lc supplement templates for `preset_name`; empty when this root
    /// ships no supplement for that preset.
    pub fn supplement_templates_for_preset(
        &self,
        preset_name: &str,
    ) -> Result<Vec<TemplateAsset>, String> {
        let dir = self.supplement_presets_dir().join(preset_name);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut templates = Vec::new();
        collect_template_assets(&self.root, &dir, &mut templates)?;
        templates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(templates)
    }

    pub fn asset_manifest_path(&self) -> PathBuf {
        self.root.join(ASSET_MANIFEST_PATH)
    }

    pub fn data_inventory(&self) -> Result<Vec<DataFileSummary>, String> {
        let mut summaries = Vec::new();
        let data_dir = self.data_dir();
        let entries = sorted_file_entries(&data_dir)?;

        for path in entries {
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some((category, language)) = parse_data_file_name(file_name) else {
                continue;
            };
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let bytes = text.len() as u64;
            let item_count =
                json_array_len(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            summaries.push(DataFileSummary {
                category,
                language,
                path: relative_to_root(&self.root, &path),
                bytes,
                item_count,
            });
        }

        summaries.sort_by(|left, right| {
            left.category
                .cmp(&right.category)
                .then(left.language.cmp(&right.language))
        });
        Ok(summaries)
    }

    pub fn data_counts_by_category_language(
        &self,
    ) -> Result<BTreeMap<String, BTreeMap<String, usize>>, String> {
        let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for summary in self.data_inventory()? {
            counts
                .entry(summary.category)
                .or_default()
                .insert(summary.language, summary.item_count);
        }
        Ok(counts)
    }

    pub fn validate_required_localized_data(
        &self,
        language: &str,
    ) -> Result<(), MissingLocalizedDataError> {
        let missing_categories = REQUIRED_LOCALIZED_DATA_CATEGORIES
            .iter()
            .filter(|category| {
                !self
                    .data_dir()
                    .join(format!("{category}.{language}.json"))
                    .is_file()
            })
            .map(|category| (*category).to_string())
            .collect::<Vec<_>>();
        if missing_categories.is_empty() {
            Ok(())
        } else {
            Err(MissingLocalizedDataError {
                language: language.to_string(),
                missing_categories,
            })
        }
    }

    pub fn list_system_presets(&self) -> Result<Vec<SystemPresetMetadata>, String> {
        let path = self.system_presets_dir().join("systempresets.json");
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        parse_system_presets(&text)
    }

    pub fn preset_config(&self, preset_name: &str) -> Result<PresetConfig, String> {
        let preset_dir = self.preset_dir(preset_name);
        let config_path = preset_dir.join("config.json");
        let text = fs::read_to_string(&config_path)
            .map_err(|error| format!("failed to read {}: {error}", config_path.display()))?;
        parse_preset_config(&text)
    }

    pub fn load_preset_templates(&self, preset_name: &str) -> Result<LoadedPresetAssets, String> {
        let config = self.preset_config(preset_name)?;
        let templates = self.template_assets_for_preset(preset_name)?;
        let counts = count_templates_by_kind(&templates);

        for required in REQUIRED_TEMPLATE_KINDS {
            if counts.get(required.as_str()).copied().unwrap_or_default() == 0 {
                return Err(format!(
                    "preset '{preset_name}' is missing required {} templates",
                    required.as_str()
                ));
            }
        }

        Ok(LoadedPresetAssets {
            preset_name: preset_name.to_string(),
            config,
            templates,
        })
    }

    pub fn template_assets_for_preset(
        &self,
        preset_name: &str,
    ) -> Result<Vec<TemplateAsset>, String> {
        let preset_dir = self.preset_dir(preset_name);
        if !preset_dir.is_dir() {
            return Err(format!("missing preset directory {}", preset_dir.display()));
        }

        let mut templates = Vec::new();
        collect_template_assets(&self.root, &preset_dir, &mut templates)?;
        templates.sort_by(|left, right| {
            left.kind
                .as_str()
                .cmp(right.kind.as_str())
                .then(left.relative_path.cmp(&right.relative_path))
        });
        Ok(templates)
    }

    pub fn template_counts_for_preset(
        &self,
        preset_name: &str,
    ) -> Result<BTreeMap<String, usize>, String> {
        let templates = self.template_assets_for_preset(preset_name)?;
        Ok(count_templates_by_kind(&templates))
    }

    pub fn controller_image_names(
        &self,
        preset_name: &str,
        language: &str,
    ) -> Result<Vec<String>, String> {
        let tooltip_dir = self.preset_dir(preset_name).join("Tooltips");
        if !tooltip_dir.is_dir() {
            return Err(format!(
                "missing tooltip directory {}",
                tooltip_dir.display()
            ));
        }

        let mut names = Vec::new();
        names.extend(select_controller_images(
            &tooltip_dir,
            "tooltip_gc_",
            language,
        )?);
        names.extend(select_controller_images(
            &tooltip_dir,
            "tooltip_kb_",
            language,
        )?);
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub fn active_controller_image_names(
        &self,
        preset_name: &str,
        language: &str,
        configured_names: &[String],
    ) -> Result<Vec<String>, String> {
        let available = self.controller_image_names(preset_name, language)?;
        if configured_names.is_empty() {
            return Ok(Vec::new());
        }

        let mut active = Vec::new();
        for image in available {
            if configured_names
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(&image))
            {
                active.push(image);
            }
        }
        active.sort();
        active.dedup();
        Ok(active)
    }

    /// Resolve a preset directory: an extracted user preset under
    /// `Images/{name}` wins over the vendored
    /// `assets/original/systempresets/{name}` tree; the vendored copy stays
    /// the fallback so every name the
    /// shipped catalog serves keeps resolving when nothing was extracted.
    /// Every preset consumer — `preset_config`, template loading,
    /// controller image enumeration — goes through this single seam, so a
    /// selected extracted preset is runtime-usable end to end.
    fn preset_dir(&self, preset_name: &str) -> PathBuf {
        let extracted = self.extracted_presets_dir().join(preset_name);
        if extracted.is_dir() {
            return extracted;
        }
        self.system_presets_dir().join(preset_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFileSummary {
    pub category: String,
    pub language: String,
    pub path: String,
    pub bytes: u64,
    pub item_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingLocalizedDataError {
    pub language: String,
    pub missing_categories: Vec<String>,
}

impl std::fmt::Display for MissingLocalizedDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "missing localized data for {}: {}",
            self.language,
            self.missing_categories.join(", ")
        )
    }
}

impl std::error::Error for MissingLocalizedDataError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPresetMetadata {
    pub file_name: String,
    pub preset_name: String,
    pub resolution: String,
    pub config: String,
    pub status: String,
    pub tooltip_height_type: Option<u32>,
    pub tooltip_width: Option<u32>,
    pub threshold_min: Option<u32>,
    pub threshold_max: Option<u32>,
    pub ready_languages: Vec<String>,
    /// Raw upstream `BrightnessThreshold` display string (e.g. "70 / 255").
    /// The settings description panel binds the catalog strings verbatim
    /// (`SettingsView.xaml:303-327`), so the raw text is carried alongside
    /// the typed numeric fields above.
    pub brightness_threshold: String,
    pub affix_area_height_offset_top: String,
    pub affix_area_height_offset_bottom: String,
    pub affix_aspect_area_width_offset: String,
    pub aspect_area_height_offset_top: String,
    pub paragon_node_size: String,
    pub paragon_node_size_collapsed: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetConfig {
    pub affix_area_height_offset_top: i32,
    pub affix_area_height_offset_bottom: i32,
    pub affix_aspect_area_width_offset: i32,
    pub aspect_area_height_offset_top: i32,
    pub paragon_left_offset_collapsed: i32,
    pub paragon_node_size: i32,
    pub paragon_node_size_collapsed: i32,
    pub paragon_top_offset_collapsed: i32,
    pub threshold_min: i32,
    pub threshold_max: i32,
    pub tooltip_height_type: i32,
    pub tooltip_width: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Tooltip,
    AffixMarker,
    AspectMarker,
    SocketMarker,
    SplitterMarker,
    Other,
}

impl TemplateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tooltip => "tooltip",
            Self::AffixMarker => "affix_marker",
            Self::AspectMarker => "aspect_marker",
            Self::SocketMarker => "socket_marker",
            Self::SplitterMarker => "splitter_marker",
            Self::Other => "other",
        }
    }
}

const REQUIRED_TEMPLATE_KINDS: &[TemplateKind] = &[
    TemplateKind::Tooltip,
    TemplateKind::AffixMarker,
    TemplateKind::AspectMarker,
    TemplateKind::SocketMarker,
    TemplateKind::SplitterMarker,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateAsset {
    pub kind: TemplateKind,
    pub file_name: String,
    pub relative_path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPresetAssets {
    pub preset_name: String,
    pub config: PresetConfig,
    pub templates: Vec<TemplateAsset>,
}

fn parse_data_file_name(file_name: &str) -> Option<(String, String)> {
    let stem = file_name.strip_suffix(".json")?;
    let (category, language) = stem.rsplit_once('.')?;
    if DATA_CATEGORIES.contains(&category) && LANGUAGES.contains(&language) {
        Some((category.to_string(), language.to_string()))
    } else {
        None
    }
}

fn sorted_file_entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read entry in {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn sorted_dir_entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read entry in {}: {error}", dir.display()))?;
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn relative_to_root(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}
/// Parse an upstream `systempresets.json` catalog from raw text.
///
/// Public entry point for callers that keep the catalog at a location
/// outside an [`AssetRepository`] root (the settings screen resolves the
/// vendored tree CWD-first / workspace-root-first).
pub fn parse_system_preset_catalog(text: &str) -> Result<Vec<SystemPresetMetadata>, String> {
    parse_system_presets(text)
}

fn parse_system_presets(text: &str) -> Result<Vec<SystemPresetMetadata>, String> {
    let items: Vec<Value> = serde_json::from_str(text)
        .map_err(|error| format!("systempresets.json is not a JSON array: {error}"))?;
    let mut presets = Vec::new();
    for object in items.iter().filter_map(Value::as_object) {
        let field = |key: &str| object.get(key).map(json_text).unwrap_or_default();
        let file_name = object
            .get("FileName")
            .and_then(Value::as_str)
            .ok_or_else(|| "system preset entry is missing FileName".to_string())?
            .to_string();
        let preset_name = file_name
            .strip_suffix(".zip")
            .unwrap_or(file_name.as_str())
            .to_string();
        let ready_languages = LANGUAGES
            .iter()
            .filter(|language| {
                object
                    .get(&format!("LanguageReady{language}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|language| (*language).to_string())
            .collect();

        presets.push(SystemPresetMetadata {
            preset_name,
            resolution: field("Resolution"),
            config: field("Config"),
            status: field("Status"),
            tooltip_height_type: leading_u32(&field("TooltipHeightType")),
            tooltip_width: leading_u32(&field("TooltipWidth")),
            // "70 / 255": the minimum is the leading number.
            threshold_min: leading_u32(&field("BrightnessThreshold")),
            threshold_max: Some(255),
            ready_languages,
            brightness_threshold: field("BrightnessThreshold"),
            affix_area_height_offset_top: field("AffixAreaHeightOffsetTop"),
            affix_area_height_offset_bottom: field("AffixAreaHeightOffsetBottom"),
            affix_aspect_area_width_offset: field("AffixAspectAreaWidthOffset"),
            aspect_area_height_offset_top: field("AspectAreaHeightOffsetTop"),
            paragon_node_size: field("ParagonNodeSize"),
            paragon_node_size_collapsed: field("ParagonNodeSizeCollapsed"),
            file_name,
        });
    }
    presets.sort_by(|left, right| left.preset_name.cmp(&right.preset_name));
    Ok(presets)
}

fn parse_preset_config(text: &str) -> Result<PresetConfig, String> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("preset config is not valid JSON: {error}"))?;
    let field = |key: &str| {
        document
            .get(key)
            .and_then(|value| match value {
                Value::Number(number) => number.as_i64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            })
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("missing numeric field '{key}'"))
    };
    Ok(PresetConfig {
        affix_area_height_offset_top: field("AffixAreaHeightOffsetTop")?,
        affix_area_height_offset_bottom: field("AffixAreaHeightOffsetBottom")?,
        affix_aspect_area_width_offset: field("AffixAspectAreaWidthOffset")?,
        aspect_area_height_offset_top: field("AspectAreaHeightOffsetTop")?,
        paragon_left_offset_collapsed: field("ParagonLeftOffsetCollapsed")?,
        paragon_node_size: field("ParagonNodeSize")?,
        paragon_node_size_collapsed: field("ParagonNodeSizeCollapsed")?,
        paragon_top_offset_collapsed: field("ParagonTopOffsetCollapsed")?,
        threshold_min: field("ThresholdMin")?,
        threshold_max: field("ThresholdMax")?,
        tooltip_height_type: field("TooltipHeightType")?,
        tooltip_width: field("TooltipWidth")?,
    })
}

/// The number of items of a top-level JSON array.
fn json_array_len(text: &str) -> Result<usize, String> {
    serde_json::from_str::<Vec<serde::de::IgnoredAny>>(text)
        .map(|items| items.len())
        .map_err(|error| format!("not a JSON array: {error}"))
}

/// A string or number field as text.
fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// The leading unsigned integer of `text` (`"70 / 255"` → 70).
fn leading_u32(text: &str) -> Option<u32> {
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}
fn collect_template_assets(
    root: &Path,
    dir: &Path,
    templates: &mut Vec<TemplateAsset>,
) -> Result<(), String> {
    for path in sorted_dir_entries(dir)? {
        if path.is_dir() {
            collect_template_assets(root, &path, templates)?;
            continue;
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let lowercase = file_name.to_ascii_lowercase();
        let kind = classify_template_file(&lowercase);
        if kind == TemplateKind::Other {
            continue;
        }
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        templates.push(TemplateAsset {
            kind,
            file_name: lowercase,
            relative_path: relative_to_root(root, &path),
            bytes: metadata.len(),
        });
    }
    Ok(())
}

fn classify_template_file(lowercase_file_name: &str) -> TemplateKind {
    if lowercase_file_name.starts_with("tooltip_") {
        TemplateKind::Tooltip
    } else if lowercase_file_name.starts_with("dot-affixes_") {
        TemplateKind::AffixMarker
    } else if lowercase_file_name.starts_with("dot-aspects_") {
        TemplateKind::AspectMarker
    } else if lowercase_file_name.starts_with("dot-socket_") {
        TemplateKind::SocketMarker
    } else if lowercase_file_name.contains("dot-splitter") {
        TemplateKind::SplitterMarker
    } else {
        TemplateKind::Other
    }
}

fn count_templates_by_kind(templates: &[TemplateAsset]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for template in templates {
        *counts
            .entry(template.kind.as_str().to_string())
            .or_insert(0) += 1;
    }
    counts
}

fn select_controller_images(
    tooltip_dir: &Path,
    prefix: &str,
    language: &str,
) -> Result<Vec<String>, String> {
    let mut matching = Vec::new();
    for path in sorted_file_entries(tooltip_dir)? {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let lowercase = file_name.to_ascii_lowercase();
        if lowercase.contains(prefix) {
            matching.push(file_name.to_string());
        }
    }

    let language_lower = language.to_ascii_lowercase();
    let language_specific_count = matching
        .iter()
        .filter(|name| name.to_ascii_lowercase().contains(&language_lower))
        .count();

    let mut selected = Vec::new();
    for name in matching {
        let lower = name.to_ascii_lowercase();
        let keep = if language_specific_count > 0 {
            lower.contains(&language_lower)
        } else {
            !LANGUAGES
                .iter()
                .any(|language| lower.contains(&language.to_ascii_lowercase()))
        };
        if keep {
            selected.push(lower);
        }
    }
    selected.sort();
    selected.dedup();
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitter_template_classification_matches_upstream_contains_rule() {
        assert_eq!(
            classify_template_file("dot-splitter.png"),
            TemplateKind::SplitterMarker
        );
        assert_eq!(
            classify_template_file("dot-splitter_top.png"),
            TemplateKind::SplitterMarker
        );
        assert_eq!(
            classify_template_file("dot-splitter_1.png"),
            TemplateKind::SplitterMarker
        );
        assert_eq!(
            classify_template_file("dot-splitter_top_1.png"),
            TemplateKind::SplitterMarker
        );
        assert_eq!(
            classify_template_file("copy-dot-splitter_1.png"),
            TemplateKind::SplitterMarker
        );
        assert_eq!(
            classify_template_file("dot-splittertop_1.png"),
            TemplateKind::SplitterMarker
        );
    }

    #[test]
    fn counts_top_level_json_array_items_without_counting_nested_arrays() {
        let text = r#"[{"A":[1,2,{"x":true}]},{"B":"[not array]"}]"#;
        assert_eq!(json_array_len(text), Ok(2));
    }

    #[test]
    fn loads_original_data_counts_by_category_and_language() {
        let repo = AssetRepository::new(workspace_root());
        let counts = match repo.data_counts_by_category_language() {
            Ok(counts) => counts,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(counts.len(), DATA_CATEGORIES.len());
        assert_eq!(
            counts
                .get("Affixes")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(894)
        );
        assert_eq!(
            counts
                .get("Aspects")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(523)
        );
        assert_eq!(
            counts
                .get("ItemTypes")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(432)
        );
        assert_eq!(
            counts
                .get("ParagonBoards")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(80)
        );
        assert_eq!(
            counts
                .get("ParagonGlyphs")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(161)
        );
        assert_eq!(
            counts
                .get("Runes")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(52)
        );
        assert_eq!(
            counts
                .get("Sigils")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(340)
        );
        assert_eq!(
            counts
                .get("Uniques")
                .and_then(|values| values.get("enUS"))
                .copied(),
            Some(308)
        );
    }

    #[test]
    fn loads_system_preset_metadata() {
        let repo = AssetRepository::new(workspace_root());
        let presets = match repo.list_system_presets() {
            Ok(presets) => presets,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(presets.len(), 10);
        let preset = match presets
            .iter()
            .find(|preset| preset.preset_name == "1080p_SMF")
        {
            Some(preset) => preset,
            None => panic!("missing 1080p_SMF preset"),
        };
        assert_eq!(preset.tooltip_width, Some(375));
        assert!(preset.ready_languages.contains(&"enUS".to_string()));
    }

    #[test]
    fn loads_required_template_groups_for_complete_preset() {
        let repo = AssetRepository::new(workspace_root());
        let loaded = match repo.load_preset_templates("1080p_SMF") {
            Ok(loaded) => loaded,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(loaded.config.tooltip_width, 375);
        let counts = count_templates_by_kind(&loaded.templates);
        assert_eq!(counts.get("tooltip").copied(), Some(9));
        assert_eq!(counts.get("affix_marker").copied(), Some(14));
        assert_eq!(counts.get("aspect_marker").copied(), Some(3));
        assert_eq!(counts.get("socket_marker").copied(), Some(6));
        assert_eq!(counts.get("splitter_marker").copied(), Some(5));
    }

    #[test]
    fn ships_the_tempered_greater_affix_supplement_for_1080p_smf() {
        let repo = AssetRepository::new(workspace_root());
        let supplement = repo
            .supplement_templates_for_preset("1080p_SMF")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            supplement
                .iter()
                .map(|asset| (asset.kind, asset.file_name.as_str()))
                .collect::<Vec<_>>(),
            vec![(
                TemplateKind::AffixMarker,
                "dot-affixes_greater_tempered.png"
            )]
        );
        assert!(repo.root().join(&supplement[0].relative_path).is_file());
        // A preset without a supplement tree has none.
        assert!(repo
            .supplement_templates_for_preset("9000p_NONE")
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty());
    }

    #[test]
    fn rejects_preset_missing_required_template_groups() {
        let repo = AssetRepository::new(workspace_root());
        let result = repo.load_preset_templates("1600p_SMF_zhCN");
        assert!(matches!(result, Err(message) if message.contains("socket_marker")));
    }

    #[test]
    fn selects_controller_images_with_language_fallback_deterministically() {
        let repo = AssetRepository::new(workspace_root());
        let generic = match repo.controller_image_names("1080p_SMF", "enUS") {
            Ok(images) => images,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            generic,
            vec![
                "tooltip_gc_square.png".to_string(),
                "tooltip_gc_x.png".to_string(),
                "tooltip_gc_y.png".to_string(),
                "tooltip_kb_all.png".to_string(),
            ]
        );

        let localized = match repo.controller_image_names("1080p_SMF", "frFR") {
            Ok(images) => images,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            localized,
            vec![
                "tooltip_gc_square.png".to_string(),
                "tooltip_gc_x.png".to_string(),
                "tooltip_gc_y.png".to_string(),
                "tooltip_kb_all_frfr.png".to_string(),
            ]
        );
    }

    #[test]
    fn filters_active_controller_images_case_insensitively() {
        let repo = AssetRepository::new(workspace_root());
        let configured = vec!["TOOLTIP_GC_X.PNG".to_string(), "missing.png".to_string()];
        let active = match repo.active_controller_image_names("1080p_SMF", "enUS", &configured) {
            Ok(active) => active,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(active, vec!["tooltip_gc_x.png".to_string()]);
    }

    #[test]
    fn extracted_user_preset_dir_takes_precedence_over_vendored_tree() {
        // A preset extracted under `Images/{name}` must win
        // over the vendored `assets/original/systempresets/{name}` copy at
        // the single `preset_dir` seam, while a name without an extracted
        // copy keeps resolving from the vendored fallback.
        let temp = unique_temp_root();
        let root = temp.path().to_path_buf();
        let config_body = |tooltip_width: u32| {
            format!(
                "{{\"AffixAreaHeightOffsetTop\": 8, \
                 \"AffixAreaHeightOffsetBottom\": 8, \
                 \"AffixAspectAreaWidthOffset\": 18, \
                 \"AspectAreaHeightOffsetTop\": 8, \
                 \"ParagonLeftOffsetCollapsed\": -200, \
                 \"ParagonNodeSize\": 29, \
                 \"ParagonNodeSizeCollapsed\": 16, \
                 \"ParagonTopOffsetCollapsed\": -50, \
                 \"ThresholdMin\": 70, \
                 \"ThresholdMax\": 255, \
                 \"TooltipHeightType\": 200, \
                 \"TooltipWidth\": {tooltip_width}}}"
            )
        };
        let vendored_config = root
            .join(SYSTEM_PRESETS_DIR)
            .join("9000p_TEST")
            .join("config.json");
        let extracted_config = root
            .join(EXTRACTED_PRESETS_DIR)
            .join("9000p_TEST")
            .join("config.json");
        std::fs::create_dir_all(vendored_config.parent().unwrap_or(&root))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::create_dir_all(extracted_config.parent().unwrap_or(&root))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&vendored_config, config_body(111))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&extracted_config, config_body(222))
            .unwrap_or_else(|error| panic!("{error}"));

        let repo = AssetRepository::new(&root);
        let extracted = repo
            .preset_config("9000p_TEST")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(extracted.tooltip_width, 222, "extracted copy must win");

        let fallback_name = "9001p_ONLYVENDORED";
        let fallback_config = root
            .join(SYSTEM_PRESETS_DIR)
            .join(fallback_name)
            .join("config.json");
        std::fs::create_dir_all(fallback_config.parent().unwrap_or(&root))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&fallback_config, config_body(111))
            .unwrap_or_else(|error| panic!("{error}"));
        let fallback = repo
            .preset_config(fallback_name)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fallback.tooltip_width, 111);
    }

    #[test]
    fn parse_system_preset_catalog_carries_raw_display_strings() {
        // The description panel binds the upstream catalog strings verbatim
        // (SettingsView.xaml:303-327); the parser must carry them through
        // instead of collapsing them into typed numerics.
        let text = r#"[
            {
             "FileName": "9000p_TEST.zip",
             "Resolution": "e.g. 9000x9000",
             "Config": "test config row",
             "TooltipHeightType": "200",
             "TooltipWidth": "375",
             "BrightnessThreshold": "70 / 255",
             "AffixAreaHeightOffsetTop": "8",
             "AffixAreaHeightOffsetBottom": "9",
             "AffixAspectAreaWidthOffset": "18",
             "AspectAreaHeightOffsetTop": "7",
             "ParagonNodeSize": "29",
             "ParagonNodeSizeCollapsed": "16",
             "Status": "ready",
             "LanguageReadydeDE": true,
             "LanguageReadyenUS": false
            }
        ]"#;
        let mut presets =
            parse_system_preset_catalog(text).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(presets.len(), 1);
        let preset = presets.pop().unwrap_or_else(|| panic!("empty catalog"));
        assert_eq!(preset.resolution, "e.g. 9000x9000");
        assert_eq!(preset.brightness_threshold, "70 / 255");
        assert_eq!(preset.affix_area_height_offset_top, "8");
        assert_eq!(preset.affix_area_height_offset_bottom, "9");
        assert_eq!(preset.affix_aspect_area_width_offset, "18");
        assert_eq!(preset.aspect_area_height_offset_top, "7");
        assert_eq!(preset.paragon_node_size, "29");
        assert_eq!(preset.paragon_node_size_collapsed, "16");
        assert!(preset.ready_languages.contains(&"deDE".to_string()));
        assert!(!preset.ready_languages.contains(&"enUS".to_string()));
    }

    #[test]
    fn validates_required_localized_data_and_reports_missing_categories_structurally() {
        let temp = unique_temp_root();
        let root = temp.path().to_path_buf();
        let data_dir = root.join(DATA_DIR);
        std::fs::create_dir_all(&data_dir).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(data_dir.join("Affixes.xxYY.json"), "[]")
            .unwrap_or_else(|error| panic!("{error}"));

        let repo = AssetRepository::new(&root);
        let error = match repo.validate_required_localized_data("xxYY") {
            Ok(()) => panic!("validation should fail"),
            Err(error) => error,
        };

        assert_eq!(error.language, "xxYY");
        assert_eq!(
            error.missing_categories,
            vec![
                "Aspects".to_string(),
                "ItemTypes".to_string(),
                "ParagonBoards".to_string(),
                "ParagonGlyphs".to_string(),
                "Runes".to_string(),
                "Sigils".to_string(),
                "Uniques".to_string(),
            ]
        );
    }

    fn workspace_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        match manifest_dir.parent().and_then(|crates| crates.parent()) {
            Some(root) => root.to_path_buf(),
            None => panic!("could not resolve workspace root"),
        }
    }

    fn unique_temp_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("d4lc-assets-test-")
            .tempdir()
            .unwrap_or_else(|error| panic!("{error}"))
    }
}
