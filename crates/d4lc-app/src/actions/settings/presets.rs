//! System and community preset selection, preset details and the
//! preset option lists.

use std::path::{Path, PathBuf};

use d4lc_gui::SettingsPanelState;
use slint::{ModelRc, SharedString, VecModel};

use super::panel::persist_settings;
use super::preset_download::PRESETS_IMAGES_SUBDIR;
use crate::app_state::AppState;
use crate::slint_generated::AppWindow;

/// Handle the settings-screen system preset selection callback.
pub fn handle_preset_selected(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    preset_name: &str,
) -> Result<(), String> {
    let preset_name = preset_name.trim();
    if preset_name.is_empty() {
        return Err("System preset name must not be empty.".to_string());
    }

    state.shell.settings.selected_system_preset = preset_name.to_string();
    seed_settings_from_preset(&mut state.shell.settings, config_root, preset_name);
    state.bump_rules_input_revision();
    persist_settings(state, config_root)?;
    state.rebuild_overlay_descriptor_state();
    apply_settings_preset_projection(
        app,
        state,
        config_root,
        Some(&ensure_zip_file_name(preset_name)),
        false,
    );
    crate::actions::debug::project_debug_stepper_values(app, state);
    Ok(())
}

/// Seed the preset-derived settings from the newly selected system
/// preset's config (upstream `SettingsViewModel.cs:258-270`): the four OCR
/// area offsets, the four paragon board values, `ThresholdMin`,
/// `ThresholdMax`, `TooltipWidth` and `TooltipMaxHeight` (the preset's
/// `TooltipHeightType`). The runtime and the paragon overlay read these
/// values from the stored settings, so the seed keeps a preset switch
/// effective — upstream performs the same seed whenever the preset changes
/// with default-config loading enabled, which is the port's only mode. A
/// preset config value that cannot represent the stored type (a negative
/// width, a threshold above 255) leaves the previously stored value in
/// place.
fn seed_settings_from_preset(
    settings: &mut SettingsPanelState,
    config_root: &Path,
    preset_name: &str,
) {
    let Ok(config) = d4lc_assets::AssetRepository::new(config_root).preset_config(preset_name)
    else {
        return;
    };
    if let Ok(width) = u32::try_from(config.tooltip_width) {
        settings.tooltip_width = width;
    }
    if let Ok(max_height) = u32::try_from(config.tooltip_height_type) {
        settings.tooltip_max_height = max_height;
    }
    settings.affix_area_height_offset_top = config.affix_area_height_offset_top;
    settings.affix_area_height_offset_bottom = config.affix_area_height_offset_bottom;
    settings.affix_aspect_area_width_offset = config.affix_aspect_area_width_offset;
    settings.aspect_area_height_offset_top = config.aspect_area_height_offset_top;
    settings.paragon_left_offset_collapsed = config.paragon_left_offset_collapsed;
    if let Ok(node_size) = u32::try_from(config.paragon_node_size) {
        settings.paragon_node_size = node_size;
    }
    if let Ok(node_size) = u32::try_from(config.paragon_node_size_collapsed) {
        settings.paragon_node_size_collapsed = node_size;
    }
    settings.paragon_top_offset_collapsed = config.paragon_top_offset_collapsed;
    if let Ok(threshold) = u8::try_from(config.threshold_min) {
        settings.threshold_min = threshold;
    }
    if let Ok(threshold) = u8::try_from(config.threshold_max) {
        settings.threshold_max = threshold;
    }
}

/// Project the persisted preset-selection state onto the SettingsScreen root
/// properties.
pub fn apply_settings_preset_projection(
    app: &AppWindow,
    state: &AppState,
    config_root: &Path,
    selected_community: Option<&str>,
    download_success: bool,
) {
    let selected_system = state.shell.settings.selected_system_preset.clone();
    let system_presets = system_preset_options(config_root, &selected_system)
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    app.set_system_presets(ModelRc::new(VecModel::from(system_presets)));
    app.set_selected_system_preset(selected_system.clone().into());

    let catalog = state
        .system_preset_catalog
        .as_ref()
        .map(|catalog| catalog.entries.as_slice());
    let community_presets = community_preset_options(config_root, &selected_system, catalog);
    let selected_community = normalize_selected_community_preset(
        selected_community,
        &community_presets,
        &selected_system,
    );
    let community_presets_model = community_presets
        .iter()
        .cloned()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    app.set_community_presets(ModelRc::new(VecModel::from(community_presets_model)));
    app.set_selected_community_preset(selected_community.clone().into());
    app.set_preset_download_success(download_success);
    project_preset_details(
        app,
        config_root,
        &selected_community,
        &selected_system,
        catalog,
    );
}

/// One display row set for the SettingsScreen description panel
/// (`PresetDetail` slint struct, `SettingsView.xaml:277-401` parity).
/// Every value is the raw display string upstream binds; the panel shows
/// an empty cell wherever the metadata does not provide a value.
struct PresetDetailData {
    resolution: String,
    config: String,
    tooltip_width: String,
    tooltip_height_type: String,
    brightness_threshold: String,
    affix_area_height_offset_top: String,
    affix_area_height_offset_bottom: String,
    affix_aspect_area_width_offset: String,
    aspect_area_height_offset_top: String,
    paragon_node_size: String,
    paragon_node_size_collapsed: String,
    ready_languages: Vec<String>,
}

/// Collect the description-panel data for the selected preset.
///
/// Lookup order mirrors the metadata that actually exists for a preset:
///
/// 1. the vendored `systempresets.json` catalog entry for the selected
///    zip (the upstream `SelectedCommunityPreset` — resolution/config
///    display rows, thresholds, offsets, node sizes and the 14
///    `LanguageReady*` flags live there);
/// 2. the preset's extracted-or-vendored `config.json`
///    ([`d4lc_assets::AssetRepository::preset_config`] — for a community
///    zip outside the vendored catalog this still yields the numeric
///    rows the extracted config provides; display-only rows stay empty
///    and the language flags stay not-ready, exactly "where metadata
///    provides them");
/// 3. empty defaults when neither source knows the preset.
fn preset_detail_data(
    config_root: &Path,
    zip_name: &str,
    preset_name: &str,
    catalog: Option<&[d4lc_assets::SystemPresetMetadata]>,
) -> PresetDetailData {
    let empty = || PresetDetailData {
        resolution: String::new(),
        config: String::new(),
        tooltip_width: String::new(),
        tooltip_height_type: String::new(),
        brightness_threshold: String::new(),
        affix_area_height_offset_top: String::new(),
        affix_area_height_offset_bottom: String::new(),
        affix_aspect_area_width_offset: String::new(),
        aspect_area_height_offset_top: String::new(),
        paragon_node_size: String::new(),
        paragon_node_size_collapsed: String::new(),
        ready_languages: Vec::new(),
    };

    // 1. The catalog of this session (the fetched upstream list, or the
    //    bundled one after a failed fetch), else the bundled catalog read
    //    through the same CWD-first / workspace-root-first seam the preset
    //    lists use.
    let bundled;
    let catalog = match catalog {
        Some(catalog) => Some(catalog),
        None => {
            bundled = bundled_system_preset_catalog();
            bundled.as_deref()
        }
    };
    if let Some(metadata) = catalog.and_then(|catalog| {
        catalog
            .iter()
            .find(|metadata| metadata.file_name == zip_name)
            .or_else(|| {
                catalog
                    .iter()
                    .find(|metadata| metadata.preset_name == preset_name)
            })
    }) {
        return PresetDetailData {
            resolution: metadata.resolution.clone(),
            config: metadata.config.clone(),
            tooltip_width: metadata
                .tooltip_width
                .map(|value| value.to_string())
                .unwrap_or_default(),
            tooltip_height_type: metadata
                .tooltip_height_type
                .map(|value| value.to_string())
                .unwrap_or_default(),
            brightness_threshold: metadata.brightness_threshold.clone(),
            affix_area_height_offset_top: metadata.affix_area_height_offset_top.clone(),
            affix_area_height_offset_bottom: metadata.affix_area_height_offset_bottom.clone(),
            affix_aspect_area_width_offset: metadata.affix_aspect_area_width_offset.clone(),
            aspect_area_height_offset_top: metadata.aspect_area_height_offset_top.clone(),
            paragon_node_size: metadata.paragon_node_size.clone(),
            paragon_node_size_collapsed: metadata.paragon_node_size_collapsed.clone(),
            ready_languages: metadata.ready_languages.clone(),
        };
    }

    // 2. Extracted/vendored preset config (extracted takes precedence —
    //    the same seam the runtime template resolution uses).
    let config = d4lc_assets::AssetRepository::new(config_root)
        .preset_config(preset_name)
        .ok();
    let Some(config) = config else {
        return empty();
    };
    PresetDetailData {
        resolution: String::new(),
        config: String::new(),
        tooltip_width: config.tooltip_width.to_string(),
        tooltip_height_type: config.tooltip_height_type.to_string(),
        brightness_threshold: format!("{} / {}", config.threshold_min, config.threshold_max),
        affix_area_height_offset_top: config.affix_area_height_offset_top.to_string(),
        affix_area_height_offset_bottom: config.affix_area_height_offset_bottom.to_string(),
        affix_aspect_area_width_offset: config.affix_aspect_area_width_offset.to_string(),
        aspect_area_height_offset_top: config.aspect_area_height_offset_top.to_string(),
        paragon_node_size: config.paragon_node_size.to_string(),
        paragon_node_size_collapsed: config.paragon_node_size_collapsed.to_string(),
        ready_languages: Vec::new(),
    }
}

/// Project the selected preset's metadata into the SettingsScreen
/// description panel and the 14 language-ready flags.
///
/// Upstream binds `SelectedCommunityPreset` (a `SystemPreset` catalog
/// entry) straight into the description panel
/// (`SettingsView.xaml:277-401`); d4lc reads the same metadata from the
/// vendored catalog or the preset's own extracted `config.json` at
/// every preset projection — startup, selection and post-download —
/// so the panel and flags are never permanently empty/dimmed.
fn project_preset_details(
    app: &AppWindow,
    config_root: &Path,
    selected_community: &str,
    selected_system: &str,
    catalog: Option<&[d4lc_assets::SystemPresetMetadata]>,
) {
    let zip_name = if selected_community.trim().is_empty() {
        ensure_zip_file_name(selected_system)
    } else {
        selected_community.trim().to_string()
    };
    let preset_name = zip_name
        .strip_suffix(".zip")
        .unwrap_or(&zip_name)
        .to_string();
    let data = preset_detail_data(config_root, &zip_name, &preset_name, catalog);

    let ready = |language: &str| {
        data.ready_languages
            .iter()
            .any(|ready| ready.eq_ignore_ascii_case(language))
    };

    app.set_preset_details(crate::slint_generated::PresetDetail {
        filename: zip_name.into(),
        resolution: data.resolution.into(),
        config: data.config.into(),
        tooltip_width: data.tooltip_width.into(),
        tooltip_height_type: data.tooltip_height_type.into(),
        brightness_threshold: data.brightness_threshold.into(),
        affix_area_height_offset_top: data.affix_area_height_offset_top.into(),
        affix_area_height_offset_bottom: data.affix_area_height_offset_bottom.into(),
        affix_aspect_area_width_offset: data.affix_aspect_area_width_offset.into(),
        aspect_area_height_offset_top: data.aspect_area_height_offset_top.into(),
        paragon_node_size: data.paragon_node_size.into(),
        paragon_node_size_collapsed: data.paragon_node_size_collapsed.into(),
        language_ready_de_de: ready("deDE"),
        language_ready_en_us: ready("enUS"),
        language_ready_es_es: ready("esES"),
        language_ready_es_mx: ready("esMX"),
        language_ready_fr_fr: ready("frFR"),
        language_ready_it_it: ready("itIT"),
        language_ready_ja_jp: ready("jaJP"),
        language_ready_ko_kr: ready("koKR"),
        language_ready_pl_pl: ready("plPL"),
        language_ready_pt_br: ready("ptBR"),
        language_ready_ru_ru: ready("ruRU"),
        language_ready_tr_tr: ready("trTR"),
        language_ready_zh_cn: ready("zhCN"),
        language_ready_zh_tw: ready("zhTW"),
    });
}

// ── System preset option enumeration ──────────────────────────
//
// Upstream evidence (pinned v5.4.0.0, commit d508a473):
// - The sidebar "System presets" ComboBox is filled by
//   `SettingsViewModel.InitSystemPresets()`
//   (`D4Companion/ViewModels/SettingsViewModel.cs:424-449`): every extracted
//   preset directory under `Images\` whose name matches the `*p_*` glob,
//   with the previous selection restored even when its directory is missing.
// - The right-panel "Available system presets" ComboBox is filled from the
//   `downloads/systempresets/systempresets.json` catalog
//   (`SystemPresetManager.UpdateSystemPresetInfo`,
//   `D4Companion.Services/SystemPresetManager.cs`;
//   `SettingsViewModel.HandleSystemPresetInfoUpdatedMessage`,
//   SettingsViewModel.cs:348-355): every catalog entry with no status filter
//   (the `broken` `1600p_SMF_zhCN` entry stays selectable so it can be
//   re-downloaded).
//
// The catalog is fetched at startup (`preset_catalog`). Until the fetch
// answers, and when it fails, the vendored copy stands in for it.
// Upstream's published preset artifacts are vendored under
// `assets/original/systempresets/` — the extracted preset directories (the
// equivalent of upstream's extracted `Images\*p_*` set) and `zips/*.zip`
// (the offline mirror of the `downloads/systempresets` catalog, 1:1 with the
// vendored `systempresets.json`). Packaging copies the whole `assets/` tree
// into the bundle and starts the binary with the bundle root as CWD, so the
// repo-relative lookup resolves at runtime; in a dev checkout the
// compile-time workspace root is the fallback (the established
// `build_import_dialogs::workspace_root_dir` pattern).

/// Vendored upstream system presets, relative to the repo/bundle root.
/// The single source is `d4lc_assets` (a direct dependency since
/// The settings screen also seeds preset metadata and
/// extracts preset zips through it in every build).
const SYSTEM_PRESETS_ASSET_DIR: &str = d4lc_assets::SYSTEM_PRESETS_DIR;

/// The workspace root (dev checkout) — same derivation as
/// `build_import_dialogs::workspace_root_dir`.
fn settings_workspace_root_dir() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // d4lc-app
    root.pop(); // crates
    root
}

/// Resolve the vendored `assets/original/systempresets` directory: CWD-first
/// (installed bundle layout), then the compile-time dev-checkout root.
/// `None` when the vendored tree exists in neither location.
pub(super) fn system_presets_asset_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(SYSTEM_PRESETS_ASSET_DIR));
    }
    candidates.push(settings_workspace_root_dir().join(SYSTEM_PRESETS_ASSET_DIR));
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// Upstream extraction glob `*p_*` (`Directory.GetDirectories(directory,
/// "*p_*")`, SettingsViewModel.cs:434) — every directory name containing the
/// literal `p_` (all shipped presets: `1080p_SMF`, `2160p_SSF`, …).
fn matches_upstream_preset_glob(name: &str) -> bool {
    name.contains("p_")
}

/// Preset directory names in `dir` matching upstream's `*p_*` extraction
/// glob, sorted. Empty when the directory does not exist.
fn system_preset_directory_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return names;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if matches_upstream_preset_glob(&name) {
            names.push(name);
        }
    }
    names.sort();
    names
}

/// The vendored `systempresets.json` catalog, parsed. `None` when the file is
/// missing or unreadable.
pub(super) fn bundled_system_preset_catalog() -> Option<Vec<d4lc_assets::SystemPresetMetadata>> {
    let path = system_presets_asset_dir()?.join("systempresets.json");
    let text = std::fs::read_to_string(path).ok()?;
    d4lc_assets::parse_system_preset_catalog(&text).ok()
}

/// Zips shipped in the vendored `zips/` catalog (the offline mirror of
/// upstream's `downloads/systempresets/*.zip`), sorted.
fn bundled_system_preset_zip_names() -> Vec<String> {
    let mut names = Vec::new();
    let Some(presets_dir) = system_presets_asset_dir() else {
        return names;
    };
    let Ok(entries) = std::fs::read_dir(presets_dir.join("zips")) else {
        return names;
    };
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_zip_file_name(&name) {
            names.push(name);
        }
    }
    names.sort();
    names
}

/// Sidebar "System presets" options: every extracted preset directory —
/// the vendored shipped set plus any preset extracted under
/// `config_root/Images/` — with the persisted selection restored even when
/// its directory is missing (upstream `InitSystemPresets` parity).
fn system_preset_options(config_root: &Path, selected_system_preset: &str) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();

    for name in system_presets_asset_dir()
        .map(|dir| system_preset_directory_names(&dir))
        .unwrap_or_default()
    {
        names.insert(name);
    }
    for name in system_preset_directory_names(&config_root.join(PRESETS_IMAGES_SUBDIR)) {
        names.insert(name);
    }

    if !selected_system_preset.trim().is_empty() {
        names.insert(selected_system_preset.trim().to_string());
    }

    names.into_iter().collect()
}

/// "Available system presets" options: every zip shipped in the vendored
/// catalog plus community zips already downloaded under
/// `config_root/Images/`, with the selection's zip kept visible (upstream
/// `systempresets.json` catalog parity — every entry, no status filter).
fn community_preset_options(
    config_root: &Path,
    selected_system_preset: &str,
    catalog: Option<&[d4lc_assets::SystemPresetMetadata]>,
) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();

    match catalog {
        // The session catalog lists every entry: `CommunitySystemPresets`
        // is filled from `SystemPresets` as is (SettingsViewModel.cs:348-355).
        Some(catalog) => names.extend(catalog.iter().map(|entry| entry.file_name.clone())),
        None => names.extend(bundled_system_preset_zip_names()),
    }

    let images_dir = config_root.join(PRESETS_IMAGES_SUBDIR);
    if let Ok(entries) = std::fs::read_dir(images_dir) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if is_zip_file_name(&name) {
                names.insert(name);
            }
        }
    }

    let derived_selected = ensure_zip_file_name(selected_system_preset);
    if !derived_selected.trim().is_empty() {
        names.insert(derived_selected);
    }

    names.into_iter().collect()
}

fn normalize_selected_community_preset(
    selected_community: Option<&str>,
    community_presets: &[String],
    selected_system_preset: &str,
) -> String {
    let selected_community = selected_community
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ensure_zip_file_name(selected_system_preset));

    if community_presets
        .iter()
        .any(|preset| preset == &selected_community)
    {
        selected_community
    } else {
        community_presets.first().cloned().unwrap_or_default()
    }
}

fn ensure_zip_file_name(preset_name: &str) -> String {
    let preset_name = preset_name.trim();
    if preset_name.is_empty() {
        return String::new();
    }
    if is_zip_file_name(preset_name) {
        preset_name.to_string()
    } else {
        format!("{preset_name}.zip")
    }
}

fn is_zip_file_name(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().ends_with(".zip")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::preset_download::PRESETS_IMAGES_SUBDIR;
    use crate::actions::settings::test_support::{load_persisted, with_temp_config_root};

    #[test]
    fn handle_preset_selected_persists_system_preset_and_projects_zip_selection() {
        with_temp_config_root(|config_root| {
            let app = crate::test_window::new_app_window();
            let mut state = crate::app_state::AppState::default_startup();

            handle_preset_selected(&app, &mut state, config_root, "1080p_SMF")
                .expect("preset selection should persist");

            assert_eq!(state.shell.settings.selected_system_preset, "1080p_SMF");
            assert_eq!(app.get_selected_system_preset().as_str(), "1080p_SMF");
            assert_eq!(
                app.get_selected_community_preset().as_str(),
                "1080p_SMF.zip"
            );

            let persisted = load_persisted(config_root);
            assert_eq!(persisted.selected_system_preset, "1080p_SMF");
        });
    }

    /// A preset switch seeds every value of the preset's config, like
    /// upstream's `SelectedSystemPreset` setter: the OCR area offsets, the
    /// paragon board values, the thresholds and the tooltip geometry.
    #[test]
    fn a_preset_switch_seeds_every_preset_config_value() {
        with_temp_config_root(|config_root| {
            let preset_dir = config_root
                .join(d4lc_assets::EXTRACTED_PRESETS_DIR)
                .join("1080p_TEST");
            std::fs::create_dir_all(&preset_dir).expect("preset dir");
            std::fs::write(
                preset_dir.join("config.json"),
                r#"{
                  "AffixAreaHeightOffsetTop": 1,
                  "AffixAreaHeightOffsetBottom": 2,
                  "AffixAspectAreaWidthOffset": 3,
                  "AspectAreaHeightOffsetTop": 4,
                  "ParagonLeftOffsetCollapsed": -123,
                  "ParagonNodeSize": 29,
                  "ParagonNodeSizeCollapsed": 16,
                  "ParagonTopOffsetCollapsed": -45,
                  "ThresholdMin": 66,
                  "ThresholdMax": 250,
                  "TooltipHeightType": 210,
                  "TooltipWidth": 377
                }"#,
            )
            .expect("preset config");
            let mut settings = crate::app_state::AppState::default_startup().shell.settings;

            seed_settings_from_preset(&mut settings, config_root, "1080p_TEST");

            assert_eq!(settings.affix_area_height_offset_top, 1);
            assert_eq!(settings.affix_area_height_offset_bottom, 2);
            assert_eq!(settings.affix_aspect_area_width_offset, 3);
            assert_eq!(settings.aspect_area_height_offset_top, 4);
            assert_eq!(settings.paragon_left_offset_collapsed, -123);
            assert_eq!(settings.paragon_node_size, 29);
            assert_eq!(settings.paragon_node_size_collapsed, 16);
            assert_eq!(settings.paragon_top_offset_collapsed, -45);
            assert_eq!(settings.threshold_min, 66);
            assert_eq!(settings.threshold_max, 250);
            assert_eq!(settings.tooltip_max_height, 210);
            assert_eq!(settings.tooltip_width, 377);
        });
    }

    #[test]
    fn apply_settings_preset_projection_includes_downloaded_zip_files() {
        with_temp_config_root(|config_root| {
            let app = crate::test_window::new_app_window();
            let images_dir = config_root.join(PRESETS_IMAGES_SUBDIR);
            std::fs::create_dir_all(&images_dir).expect("images dir");
            std::fs::write(images_dir.join("1440p_SSF_zhCN.zip"), b"zip").expect("zip fixture");

            let mut state = crate::app_state::AppState::default_startup();
            state.shell.settings.selected_system_preset = "1080p_SMF".to_string();

            apply_settings_preset_projection(
                &app,
                &state,
                config_root,
                Some("1440p_SSF_zhCN.zip"),
                true,
            );

            assert_eq!(
                app.get_selected_community_preset().as_str(),
                "1440p_SSF_zhCN.zip"
            );
            assert!(app.get_preset_download_success());
        });
    }

    /// The description panel data is sourced from the
    /// vendored `systempresets.json` catalog entry of the selected
    /// community preset. The expected values are re-parsed from the
    /// vendored catalog file itself (independent oracle), so the test
    /// pins the metadata mapping, not its own constants. Window-free by
    /// design: creating an `AppWindow` in a parallel unit test races the
    /// real-input harness's `set_platform` (AlreadySet), so the
    /// window-side projection (`project_preset_details` → root property
    /// → panel) is covered by the real-input harness instead.
    #[test]
    fn preset_detail_data_seeds_from_vendored_catalog() {
        with_temp_config_root(|config_root| {
            let catalog_text = std::fs::read_to_string(
                system_presets_asset_dir()
                    .unwrap_or_else(|| panic!("vendored systempresets dir must exist"))
                    .join("systempresets.json"),
            )
            .expect("read vendored systempresets.json");
            let metadata = d4lc_assets::parse_system_preset_catalog(&catalog_text)
                .expect("parse vendored catalog")
                .into_iter()
                .find(|entry| entry.file_name == "1080p_SMF.zip")
                .expect("1080p_SMF.zip in the vendored catalog");

            let data = preset_detail_data(config_root, "1080p_SMF.zip", "1080p_SMF", None);
            assert_eq!(data.resolution, metadata.resolution);
            assert_eq!(data.config, metadata.config);
            assert_eq!(
                data.tooltip_width,
                metadata
                    .tooltip_width
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            );
            assert_eq!(data.brightness_threshold, metadata.brightness_threshold);
            assert_eq!(data.paragon_node_size, metadata.paragon_node_size);
            assert_eq!(
                data.ready_languages.contains(&"deDE".to_string()),
                metadata.ready_languages.iter().any(|lang| lang == "deDE"),
                "DE readiness must mirror the catalog LanguageReadydeDE"
            );
            assert_eq!(
                data.ready_languages.contains(&"itIT".to_string()),
                metadata.ready_languages.iter().any(|lang| lang == "itIT"),
                "IT readiness must mirror the catalog LanguageReadyitIT"
            );
        });
    }

    /// A community zip outside the vendored catalog sources its numeric
    /// rows from the extracted preset `config.json` (extracted dirs win
    /// over the vendored tree); display-only rows stay empty and every
    /// language flag stays not-ready ("where metadata provides them").
    #[test]
    fn preset_detail_data_seeds_from_extracted_config() {
        with_temp_config_root(|config_root| {
            let images_dir = config_root.join(PRESETS_IMAGES_SUBDIR);
            std::fs::create_dir_all(&images_dir).expect("images dir");
            let preset_dir = images_dir.join("7700p_Community");
            std::fs::create_dir_all(&preset_dir).expect("extracted preset dir");
            std::fs::write(
                preset_dir.join("config.json"),
                concat!(
                    "{\"AffixAreaHeightOffsetTop\": 5, ",
                    "\"AffixAreaHeightOffsetBottom\": 6, ",
                    "\"AffixAspectAreaWidthOffset\": 7, ",
                    "\"AspectAreaHeightOffsetTop\": 8, ",
                    "\"ParagonLeftOffsetCollapsed\": -100, ",
                    "\"ParagonNodeSize\": 25, ",
                    "\"ParagonNodeSizeCollapsed\": 14, ",
                    "\"ParagonTopOffsetCollapsed\": -40, ",
                    "\"ThresholdMin\": 60, ",
                    "\"ThresholdMax\": 250, ",
                    "\"TooltipHeightType\": 180, ",
                    "\"TooltipWidth\": 444}"
                ),
            )
            .expect("extracted config fixture");

            let data =
                preset_detail_data(config_root, "7700p_Community.zip", "7700p_Community", None);
            assert_eq!(data.resolution, "", "no catalog row -> no resolution");
            assert_eq!(data.tooltip_width, "444");
            assert_eq!(data.tooltip_height_type, "180");
            assert_eq!(data.brightness_threshold, "60 / 250");
            assert_eq!(data.affix_area_height_offset_top, "5");
            assert_eq!(data.affix_area_height_offset_bottom, "6");
            assert_eq!(data.affix_aspect_area_width_offset, "7");
            assert_eq!(data.aspect_area_height_offset_top, "8");
            assert_eq!(data.paragon_node_size, "25");
            assert_eq!(data.paragon_node_size_collapsed, "14");
            assert!(
                data.ready_languages.is_empty(),
                "no catalog metadata -> languages stay not-ready"
            );
        });
    }

    //
    // ── System preset completeness ────────────────────────
    //
    // The Settings preset lists must project the *entire* shipped set, not
    // a config-derived fragment. The expected sets are enumerated here
    // directly from the vendored assets, so a projection that drops any
    // shipped preset fails these tests.
    /// Independently enumerate the vendored preset directory names (the
    /// `*p_*` dirs under `assets/original/systempresets`).
    fn vendored_preset_dir_names() -> Vec<String> {
        let dir = system_presets_asset_dir()
            .unwrap_or_else(|| panic!("vendored assets/original/systempresets must exist"));
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("read vendored systempresets dir")
            .flatten()
            .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.contains("p_"))
            .collect();
        names.sort();
        names
    }

    /// Independently enumerate the shipped catalog zips (`zips/*.zip`).
    fn vendored_preset_zip_names() -> Vec<String> {
        let dir = system_presets_asset_dir()
            .unwrap_or_else(|| panic!("vendored assets/original/systempresets must exist"))
            .join("zips");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("read vendored systempresets zips dir")
            .flatten()
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.to_ascii_lowercase().ends_with(".zip"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn system_preset_options_enumerates_every_shipped_preset() {
        with_temp_config_root(|config_root| {
            let options = system_preset_options(config_root, "");

            // Fresh config root: no user-extracted presets, no selection —
            // the list must be exactly the vendored shipped set.
            assert_eq!(options, vendored_preset_dir_names());
            // Sanity: the shipped set is the multi-preset catalog, not an
            // empty pass (config default 1440p_SMF plus at least the two
            // other resolutions shipped upstream).
            assert!(
                options.len() >= 3,
                "the shipped preset set must be fully enumerated, got: {options:?}"
            );
            assert!(
                options.contains(&d4lc_config::AppSettings::default().selected_system_preset),
                "the config default preset must be part of the enumerated set"
            );

            // The persisted selection stays visible even when its directory
            // is missing (upstream restores the previous selection).
            let options_with_selection = system_preset_options(config_root, "custom_preset_x");
            assert!(options_with_selection.contains(&"custom_preset_x".to_string()));
        });
    }

    #[test]
    fn community_preset_options_follow_the_session_catalog() {
        with_temp_config_root(|config_root| {
            let entry = |file_name: &str| {
                let text = format!(r#"[{{ "FileName": "{file_name}" }}]"#);
                d4lc_assets::parse_system_preset_catalog(&text)
                    .expect("catalog entry")
                    .remove(0)
            };
            let catalog = vec![entry("9100p_JOB.zip"), entry("9200p_JOB.zip")];

            let options = community_preset_options(config_root, "", Some(&catalog));

            // Only the session catalog's entries: the bundled zips are not
            // merged in once a catalog answered.
            assert_eq!(options, ["9100p_JOB.zip", "9200p_JOB.zip"]);

            let data =
                preset_detail_data(config_root, "9200p_JOB.zip", "9200p_JOB", Some(&catalog));
            assert!(data.resolution.is_empty());
            assert!(data.ready_languages.is_empty());
        });
    }

    #[test]
    fn community_preset_options_enumerates_every_shipped_catalog_zip() {
        with_temp_config_root(|config_root| {
            let options = community_preset_options(config_root, "", None);

            // Fresh config root: the available list must be exactly the
            // shipped catalog zips.
            assert_eq!(options, vendored_preset_zip_names());
            assert!(
                options.len() >= 3,
                "the shipped catalog must be fully enumerated, got: {options:?}"
            );

            // Cross-lock: every shipped zip is listed by the vendored
            // upstream `systempresets.json` catalog (the file upstream's
            // available list is built from).
            let catalog = system_presets_asset_dir()
                .unwrap_or_else(|| panic!("vendored systempresets dir must exist"))
                .join("systempresets.json");
            let catalog_text =
                std::fs::read_to_string(&catalog).expect("read vendored systempresets.json");
            for zip in vendored_preset_zip_names() {
                assert!(
                    catalog_text.contains(&zip),
                    "shipped zip {zip} must be listed in the vendored systempresets.json"
                );
            }

            // A downloaded-only community zip joins the shipped set.
            let images_dir = config_root.join(PRESETS_IMAGES_SUBDIR);
            std::fs::create_dir_all(&images_dir).expect("images dir");
            std::fs::write(images_dir.join("custom_community.zip"), b"zip")
                .expect("custom zip fixture");
            let options = community_preset_options(config_root, "", None);
            assert!(options.contains(&"custom_community.zip".to_string()));
            assert_eq!(
                options,
                {
                    let mut merged = vendored_preset_zip_names();
                    merged.push("custom_community.zip".to_string());
                    merged.sort();
                    merged
                },
                "downloaded zips must merge with the shipped catalog"
            );
        });
    }
}
