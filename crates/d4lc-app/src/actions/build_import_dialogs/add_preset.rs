//! Add-as-preset: the Confirm-name overlay, the Delete overlay, the
//! editor projection of an import, and the import settings.

use std::path::{Path, PathBuf};

use d4lc_config::AppSettings;

use super::dialog::{cached_build, seed_preset_models};
use super::merge_tab::handle_merge_name_done;
use super::paths::{dialog_builds_dir, resolve_asset_repo_root, resolve_source_path};
use super::{
    PROVIDER_D2CORE, PROVIDER_D4BUILDS, PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL,
    PROVIDER_MOBALYTICS,
};
use crate::app_state::{AppState, PendingVariantImport};
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Per-provider import dispatch (`CreatePresetFrom{Provider}Build` parity).
// ---------------------------------------------------------------------------

#[cfg(feature = "real-runtime")]
fn import_variant(
    provider: &str,
    repo_root: &Path,
    source_path: &Path,
    variant_name: Option<&str>,
    settings: &AppSettings,
) -> Result<d4lc_rules::ImportWorkflowResult, String> {
    use d4lc_assets::AssetRepository;
    use d4lc_rules::{
        import_d2core_build_variant, import_d4builds_build_variant, import_maxroll_build_profile,
        import_mobalytics_build_variant,
    };

    let repo = AssetRepository::new(repo_root);
    match provider {
        PROVIDER_D2CORE => import_d2core_build_variant(&repo, source_path, variant_name, settings),
        PROVIDER_D4BUILDS => {
            import_d4builds_build_variant(&repo, source_path, variant_name, settings)
        }
        PROVIDER_MAXROLL => {
            import_maxroll_build_profile(&repo, source_path, variant_name, settings)
        }
        PROVIDER_MOBALYTICS => {
            import_mobalytics_build_variant(&repo, source_path, variant_name, settings)
        }
        PROVIDER_INFINITY_BUILDS => d4lc_rules::import_infinity_builds_build_variant(
            &repo,
            source_path,
            variant_name,
            settings,
        ),
        other => Err(format!("unknown build-import provider '{other}'")),
    }
}

#[cfg(not(feature = "real-runtime"))]
fn import_variant(
    provider: &str,
    _repo_root: &Path,
    _source_path: &Path,
    _variant_name: Option<&str>,
    _settings: &AppSettings,
) -> Result<d4lc_rules::ImportWorkflowResult, String> {
    Err(format!(
        "build import requires the `real-runtime` feature for the '{provider}' provider \
         (offline-only build in this build configuration; rebuild with \
         `--features real-runtime`)"
    ))
}

// ---------------------------------------------------------------------------
// Add-as-preset (upstream `Add{Provider}BuildAsPresetExecute` +
// `SetPresetNameViewModel`): the Confirm-name overlay with the upstream
// suggestion lists and the red overwrite warning.
// ---------------------------------------------------------------------------

/// Variant-row Plus click: seed the Confirm-name overlay. D2Core offers
/// two suggestions (build, variant); every other provider four (build,
/// variant, and both combinations, `ImportAffixPresetViewModel.cs:792-968`).
pub fn handle_variant_add(
    app: &AppWindow,
    state: &mut AppState,
    provider: &str,
    variant_index: i32,
) -> Result<(), String> {
    let build = {
        let selected = match provider {
            PROVIDER_D2CORE => app.get_import_d2core_selected_build_index(),
            PROVIDER_D4BUILDS => app.get_import_d4builds_selected_build_index(),
            PROVIDER_INFINITY_BUILDS => app.get_import_infinity_selected_build_index(),
            PROVIDER_MAXROLL => app.get_import_maxroll_selected_build_index(),
            PROVIDER_MOBALYTICS => app.get_import_mobalytics_selected_build_index(),
            _ => -1,
        };
        cached_build(state, provider, selected)?
    };
    if variant_index < 0 || variant_index as usize >= build.variant_names.len() {
        return Err(format!(
            "no variant row {variant_index} for build '{}'",
            build.id
        ));
    }
    let variant_name = build.variant_names[variant_index as usize].clone();
    let suggestions: Vec<String> = if provider == PROVIDER_D2CORE {
        vec![build.name.clone(), variant_name.clone()]
    } else {
        vec![
            build.name.clone(),
            variant_name.clone(),
            format!("{} - {}", build.name, variant_name),
            format!("{} - {}", variant_name, build.name),
        ]
    };
    state.build_import.pending_import = Some(PendingVariantImport {
        provider: provider.to_string(),
        build_id: build.id.clone(),
        build_name: build.name.clone(),
        variant_name,
    });
    state.bump_rules_input_revision();
    let rows: Vec<slint::SharedString> = suggestions
        .iter()
        .map(|suggestion| suggestion.clone().into())
        .collect();
    app.set_import_name_suggestions(slint::ModelRc::new(slint::VecModel::from(rows)));
    // Initial name = first suggestion (upstream `SelectedSuggestion` setter
    // fills the TextBox).
    app.set_import_name_text(suggestions[0].clone().into());
    app.set_import_name_overwrite_warning(name_exists(state, &suggestions[0]));
    app.set_import_name_visible(true);
    Ok(())
}

/// Case-sensitive name match (upstream `ShowOverwriteWarning`).
pub(super) fn name_exists(state: &AppState, name: &str) -> bool {
    state
        .affix_preset_editor
        .presets
        .iter()
        .any(|preset| preset.name == name)
}

/// Suggestion picked in the Confirm-name combo: fills the name box
/// (upstream `SelectedSuggestion` setter).
pub fn handle_name_suggestion_selected(app: &AppWindow, state: &AppState, value: &str) {
    app.set_import_name_text(value.into());
    app.set_import_name_overwrite_warning(name_exists(state, value));
}

/// Live overwrite-warning update while the name box is typed into.
pub fn handle_name_text_changed(app: &AppWindow, state: &AppState) {
    let current = app.get_import_name_text().to_string();
    app.set_import_name_overwrite_warning(name_exists(state, &current));
}

/// Cancel the Confirm-name overlay (abandons both the add-as-preset and
/// the merge pending flows).
pub fn handle_name_cancel(app: &AppWindow, state: &mut AppState) {
    state.build_import.pending_import = None;
    state.bump_rules_input_revision();
    state.build_import.pending_merge = None;
    state.bump_rules_input_revision();
    app.set_import_name_visible(false);
}

/// Resolve the JSON file to import for a stored build row.
///
/// The descriptor's own `source_path` — the file the loader actually
/// parsed, exact even when the file stem does not mirror the build id —
/// wins. A selected source that is gone is an ERROR, never a silent
/// re-selection: importing whatever other JSON the directory carries
/// would evaluate a build the user did not choose. Only a row
/// without a usable path (a builds directory replaced under the open
/// dialog) falls back to the strict per-build identity lookup of
/// [`resolve_source_path`], which itself never substitutes another build.
fn resolve_import_source_path(
    state: &AppState,
    provider: &str,
    build_id: &str,
    builds_dir: &Path,
) -> Result<PathBuf, String> {
    let from_descriptor = state
        .build_import
        .provider_builds
        .get(provider)
        .and_then(|builds| builds.iter().find(|build| build.id == build_id))
        .map(|build| build.source_path.clone())
        .filter(|path| !path.is_empty());
    if let Some(path) = from_descriptor {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "the selected build JSON {} for '{build_id}' no longer exists; \
             refresh the build list and select the build again",
            path.display()
        ));
    }
    resolve_source_path(provider, builds_dir, build_id).ok_or_else(|| {
        format!(
            "no stored build JSON for '{build_id}' under {}; \
             refresh the build list and select the build again",
            builds_dir.display()
        )
    })
}

/// Confirm-name Done: run the add-as-preset flow — import the selected
/// variant, silently overwrite a same-name preset, persist, and re-select
/// the created preset (upstream `CreatePresetFrom{Provider}Build` +
/// `AffixManager.AddAffixPreset`). When the overlay was opened by the
/// Merge tab, the merge flow runs instead.
///
/// Like every preset-list mutation, the confirm refreshes the AffixScreen
/// preset ComboBox and the selected preset's entry models in the same
/// session (upstream `AffixPresets` / `SelectedAffixPreset` are
/// observable), so no restart is needed to see the imported preset.
pub fn handle_name_done(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    if state.build_import.pending_merge.is_some() {
        return handle_merge_name_done(app, state, config_root);
    }
    let pending = state
        .build_import
        .pending_import
        .clone()
        .ok_or_else(|| "no pending build variant to add as preset".to_string())?;
    let preset_name = app.get_import_name_text().trim().to_string();
    if preset_name.is_empty() {
        return Err("Preset name must not be empty.".to_string());
    }

    let provider = pending.provider.clone();
    let builds_dir = dialog_builds_dir(&provider, state)
        .ok_or_else(|| format!("No builds directory available for provider '{provider}'."))?;
    let source_path = resolve_import_source_path(state, &provider, &pending.build_id, &builds_dir)?;
    let repo_root = resolve_asset_repo_root().ok_or_else(|| {
        format!(
            "No localized import data available (assets/original/data). \
             Provider '{provider}' imports need the bundled asset data; \
             run from a checkout or install with the bundled assets."
        )
    })?;

    // Paragon import follows the tab's toggle (upstream reads the
    // per-provider setting at conversion time).
    let mut settings = load_app_settings(state);
    apply_paragon_toggles(&mut settings, &state.build_import.paragon_toggles);
    let result = import_variant(
        &provider,
        &repo_root,
        &source_path,
        Some(&pending.variant_name),
        &settings,
    )?;

    // The editor's `import_preset` removes a same-name preset first and
    // selects the new one (upstream `AddAffixPreset` overwrite + sort).
    // The editor is snapshotted so a failed commit restores it: the
    // import must not leave a preset that only exists in RAM.
    let editor_before = state.affix_preset_editor.clone();
    let downloaded_before = state.downloaded_paragon_routes.clone();
    let entries = build_editor_entries(&result);
    let warnings = build_editor_warnings(&result);
    state.affix_preset_editor.import_preset(
        d4lc_gui::AffixPresetEditorPreset {
            name: preset_name.clone(),
            entries,
        },
        warnings,
    );
    state.set_imported_paragon_routes(&preset_name, result.paragon_routes.clone());
    // Validate → persist → success: the imported preset and the new
    // selection are committed as one transaction, and only a successful
    // commit lets the dialog close. A failed write keeps the overlay
    // open with the error, leaves the previous applied revision in
    // place, and never claims the import was stored.
    if let Err(error) = state.persist_affix_presets_and_selection_to_config(config_root) {
        state.affix_preset_editor = editor_before;
        state.bump_rules_input_revision();
        state.downloaded_paragon_routes = downloaded_before;
        state.bump_rules_input_revision();
        return Err(format!("preset '{preset_name}' was not saved: {error}"));
    }

    // The created preset is the editor's new selection: refresh the
    // AffixScreen preset ComboBox and its entry models (upstream's
    // observable `AffixPresets` / `SelectedAffixPreset` update), then
    // re-project the dialog's own per-tab preset combo on it.
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);
    seed_preset_models(app, state);
    app.set_import_selected_preset(preset_name.clone().into());
    state.build_import.pending_import = None;
    state.bump_rules_input_revision();
    app.set_import_name_visible(false);

    // The import's outcome reaches the Logging screen (upstream's warning
    // list): a usable import, an import that had to skip entries, and a
    // failed import stay distinguishable instead of all reading "Imported".
    for warning in reported_warnings(&result) {
        state.append_log_message(import_warning_line(warning), crate::clock::hh_mm_now());
    }
    state.append_log_message(
        import_outcome_summary(&result, &preset_name),
        crate::clock::hh_mm_now(),
    );
    eprintln!(
        "[d4lc] add_as_preset: provider={provider} build={} variant={} preset={} entries={} \
         skipped={}",
        pending.build_id,
        pending.variant_name,
        preset_name,
        result.items.len(),
        result.warnings.len(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Preset combo + remove (upstream `RemoveAffixPresetNameExecute` with the
// "Delete" confirmation).
// ---------------------------------------------------------------------------

/// Remove-preset button: open the Delete confirmation overlay with the
/// upstream copy (`Are you sure you want to delete preset "{name}"`).
pub fn handle_preset_remove(app: &AppWindow) -> Result<(), String> {
    let name = app.get_import_selected_preset().to_string();
    if name.trim().is_empty() {
        return Err("No preset selected.".to_string());
    }
    app.set_import_delete_visible(true);
    Ok(())
}

/// Delete confirmed: remove the preset, persist, and select the first
/// remaining preset (upstream `AffixPresetRemovedMessage` handling). The
/// AffixScreen preset ComboBox, its selection and the entry models refresh
/// through the shared preset-model seam, then the dialog's own combo is
/// re-projected.
pub fn handle_delete_confirm(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let name = app.get_import_selected_preset().to_string();
    state.affix_preset_editor.remove_preset(&name)?;
    state.bump_rules_input_revision();
    // The removal and the fallback selection are persisted together;
    // a failed write leaves the confirmation open with the error
    // instead of reporting a deletion that never reached the disk.
    state
        .persist_affix_presets_and_selection_to_config(config_root)
        .map_err(|error| format!("preset '{name}' was not deleted on disk: {error}"))?;
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);
    seed_preset_models(app, state);
    app.set_import_delete_visible(false);
    eprintln!("[d4lc] deleted preset \"{name}\"");
    Ok(())
}

/// Delete cancelled: just close the overlay.
pub fn handle_delete_cancel(app: &AppWindow) {
    app.set_import_delete_visible(false);
}

// ---------------------------------------------------------------------------
// Per-tab toggles (upstream saves each flip immediately).
// ---------------------------------------------------------------------------

/// "Import paragon boards" toggle: update the in-memory toggles and
/// persist immediately (upstream `IsImportParagon{Provider}Enabled`
/// setter → `SaveSettings`).
pub fn handle_paragon_toggled(state: &mut AppState, provider: &str, enabled: bool) {
    let config_root = crate::actions::settings::resolve_config_root();
    apply_paragon_toggle(state, provider, enabled, &config_root);
}

fn apply_paragon_toggle(state: &mut AppState, provider: &str, enabled: bool, config_root: &Path) {
    state.build_import.paragon_toggles.set(provider, enabled);
    persist_build_import_settings(state, config_root);
}

/// Mobalytics "Show browser" toggle: update the persisted setting and
/// save immediately (upstream `IsShowBrowserMobalyticsEnabled`).
pub fn handle_show_browser_toggled(state: &mut AppState, enabled: bool) {
    state.shell.settings.mobalytics_show_browser = enabled;
    state.bump_rules_input_revision();
    let config_root = crate::actions::settings::resolve_config_root();
    persist_build_import_settings(state, &config_root);
}

/// Write the build-import-owned settings through the existing
/// load-modify-save seam (the panel-driven settings saves load from disk
/// first, so fields the panel does not model survive their saves).
///
/// The settings panel also carries the Infinity paragon flag and writes it
/// back on every panel-driven save, so it follows the toggle here; a stale
/// panel copy would revert the flag at the next unrelated settings save.
fn persist_build_import_settings(state: &mut AppState, config_root: &Path) {
    state
        .shell
        .settings
        .is_import_paragon_infinity_builds_enabled =
        state.build_import.paragon_toggles.infinity_builds;
    state.bump_rules_input_revision();
    let result = d4lc_config::load_app_settings(config_root).and_then(|mut config| {
        let toggles = &state.build_import.paragon_toggles;
        config.is_import_paragon_d2core_enabled = toggles.d2core;
        config.is_import_paragon_d4builds_enabled = toggles.d4builds;
        config.is_import_paragon_infinity_builds_enabled = toggles.infinity_builds;
        config.is_import_paragon_maxroll_enabled = toggles.maxroll;
        config.is_import_paragon_mobalytics_enabled = toggles.mobalytics;
        config.is_show_browser_mobalytics_enabled = state.shell.settings.mobalytics_show_browser;
        d4lc_config::save_app_settings(config_root, &config).map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        state.append_log_message(
            format!("build-import settings save failed: {error}"),
            crate::clock::hh_mm_now(),
        );
    }
}

// ---------------------------------------------------------------------------
// Editor entry / warning projection for the add-as-preset flow.
// ---------------------------------------------------------------------------

fn build_editor_entries(result: &d4lc_rules::ImportWorkflowResult) -> Vec<d4lc_gui::PresetEntry> {
    use d4lc_gui::PresetEntry;
    let mut entries: Vec<PresetEntry> = Vec::new();
    for affix in &result.affix_preset.affixes {
        entries.push(PresetEntry {
            kind: d4lc_gui::PresetEntryKind::Affix,
            id: affix.affix_id.clone(),
            label: affix.label.clone(),
            item_type: Some(affix.item_type.clone()),
            color: affix.color.clone(),
            // The import result carries the upstream `IsAnyType` flag and
            // the prism list; both belong to the preset entry so they
            // survive the editor, the next save and a restart.
            is_any_type: affix.is_any_type,
            is_greater: affix.is_greater,
            is_implicit: affix.is_implicit,
            is_tempered: affix.is_tempered,
            tuning_prisms: affix.tuning_prisms.clone(),
        });
    }
    for aspect in &result.aspects {
        entries.push(PresetEntry {
            kind: d4lc_gui::PresetEntryKind::Aspect,
            id: aspect.id.clone(),
            label: None,
            item_type: aspect.item_type.clone(),
            color: aspect.color.clone(),
            is_any_type: aspect.item_type.is_none(),
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        });
    }
    for unique in &result.uniques {
        entries.push(PresetEntry {
            kind: d4lc_gui::PresetEntryKind::Unique,
            id: unique.id.clone(),
            label: None,
            item_type: unique.item_type.clone(),
            color: unique.color.clone(),
            is_any_type: unique.item_type.is_none(),
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        });
    }
    for rune in &result.runes {
        entries.push(PresetEntry {
            kind: d4lc_gui::PresetEntryKind::Rune,
            id: rune.id.clone(),
            label: None,
            item_type: None,
            color: rune.color.clone(),
            is_any_type: true,
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        });
    }
    entries
}

fn build_editor_warnings(
    result: &d4lc_rules::ImportWorkflowResult,
) -> Vec<d4lc_gui::ImportWarningView> {
    reported_warnings(result)
        .map(|w| d4lc_gui::ImportWarningView {
            provider: w.provider.clone(),
            source_field: w.source_field.clone(),
            source_value: w.source_value.clone(),
            message: w.message.clone(),
            upstream_method: w.upstream_method.clone(),
        })
        .collect()
}

/// The import warnings upstream reports: the `Imported {Provider} build
/// contains unknown ...` messages of the D2Core, Maxroll and InfinityBuilds
/// conversions (`WarningOccurredMessage`). The structured warnings of the
/// other conversions (D4Builds and Mobalytics site texts that did not map)
/// stay in the import result; upstream reports nothing for them.
fn reported_warnings(
    result: &d4lc_rules::ImportWorkflowResult,
) -> impl Iterator<Item = &d4lc_rules::ImportWarning> {
    result.warnings.iter().filter(|warning| {
        warning.message.starts_with("Imported ") && warning.message.contains(" build contains ")
    })
}

/// The Logging-screen line of one import warning. Upstream reports one
/// `WarningOccurredMessage` per unknown entry, with the entry in the text
/// (`Imported Maxroll build contains unknown aspect sno: 123.`); a message
/// that does not name its entry gets the source value appended.
fn import_warning_line(warning: &d4lc_rules::ImportWarning) -> String {
    if warning.source_value.is_empty() || warning.message.contains(&warning.source_value) {
        warning.message.clone()
    } else {
        format!("{}: {}", warning.message, warning.source_value)
    }
}

/// The Logging-screen line one finished provider import reports (upstream
/// routes every `WarningOccurredMessage` to the same list).
///
/// It keeps the three outcomes apart: a fully usable import reports only
/// what became usable, an import with warnings additionally names how many
/// entries were skipped and why, and a failed import never reaches this line
/// at all (the handler returns the error before persisting). An import whose
/// warnings are dropped, or whose skipped entries are not named, is not
/// distinguishable from a clean one on the user-visible surface.
fn import_outcome_summary(result: &d4lc_rules::ImportWorkflowResult, preset_name: &str) -> String {
    let counts = format!(
        "{} affixes, {} item slots, {} aspects, {} uniques, {} runes",
        result.affix_preset.affixes.len(),
        result.items.len(),
        result.aspects.len(),
        result.uniques.len(),
        result.runes.len(),
    );
    let build = if result.source_build_name.trim().is_empty() {
        result.source_build_id.clone()
    } else {
        result.source_build_name.clone()
    };
    let variant = result
        .variant_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!(" '{name}'"))
        .unwrap_or_default();
    let warnings: Vec<&d4lc_rules::ImportWarning> = reported_warnings(result).collect();
    if warnings.is_empty() {
        return format!(
            "Imported {provider} build '{build}'{variant} as preset '{preset_name}': {counts}.",
            provider = result.provider,
        );
    }
    let reasons: Vec<String> = warnings.iter().map(|w| import_warning_line(w)).collect();
    format!(
        "Imported {provider} build '{build}'{variant} as preset '{preset_name}' with \
         {skipped} skipped entr{y}: {counts} usable. Skipped reasons: {reasons}",
        provider = result.provider,
        skipped = reasons.len(),
        y = if reasons.len() == 1 { "y" } else { "ies" },
        reasons = reasons.join("; "),
    )
}

// ---------------------------------------------------------------------------
// `AppSettings` projection helper.
//
// The d4lc-rules import APIs need an `AppSettings` value (for
// paragon toggles, selected affix language, etc). We project
// the current `SettingsPanelState` into an `AppSettings`
// using the existing `from_config` inverse: build a default
// `AppSettings` then call `apply_to_config` to overlay the
// panel state. The build-import dialog state merges its
// multi-build list and its per-provider paragon toggles so
// the import APIs see the dialog's live values.
// ---------------------------------------------------------------------------

pub(super) fn load_app_settings(state: &AppState) -> AppSettings {
    // Use the existing `apply_to_config` helper to project
    // `SettingsPanelState` onto a default `AppSettings`.
    let mut settings = AppSettings::default();
    state.shell.settings.apply_to_config(&mut settings);
    // The build-import dialog state owns the multi-build list; merge it
    // into the projected
    // settings so the import APIs see the same list.
    settings.rule_settings.multi_builds = state
        .build_import
        .multi_builds
        .iter()
        .map(|row| d4lc_config::MultiBuildRuleConfig {
            name: row.name.clone(),
            index: row.index,
            color: row.color.clone(),
        })
        .collect();
    apply_paragon_toggles(&mut settings, &state.build_import.paragon_toggles);
    settings
}

/// Overlay the dialog's live per-provider paragon toggles onto the
/// projected settings (the import conversions read the per-provider
/// `is_import_paragon_*` flags).
fn apply_paragon_toggles(
    settings: &mut AppSettings,
    toggles: &crate::app_state::ParagonImportToggles,
) {
    settings.is_import_paragon_d2core_enabled = toggles.d2core;
    settings.is_import_paragon_d4builds_enabled = toggles.d4builds;
    settings.is_import_paragon_infinity_builds_enabled = toggles.infinity_builds;
    settings.is_import_paragon_maxroll_enabled = toggles.maxroll;
    settings.is_import_paragon_mobalytics_enabled = toggles.mobalytics;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn import_warning_lines_name_the_entry_once() {
        let warning = |message: &str, value: &str| d4lc_rules::ImportWarning {
            provider: "Maxroll".to_string(),
            source_field: "Aspect[0]".to_string(),
            source_value: value.to_string(),
            message: message.to_string(),
            upstream_method: "m".to_string(),
        };
        // Upstream's text already carries the entry (BuildsManagerMaxroll.cs:388).
        assert_eq!(
            import_warning_line(&warning(
                "Imported Maxroll build contains unknown aspect sno: 123.",
                "123"
            )),
            "Imported Maxroll build contains unknown aspect sno: 123."
        );
        // A message without the entry gets it appended.
        assert_eq!(
            import_warning_line(&warning("aspect name did not map", "Bogus")),
            "aspect name did not map: Bogus"
        );
    }
    use crate::actions::build_import_dialogs::paths::{
        load_builds_for_provider, resolve_builds_dir_with_config_root, resolve_source_path,
    };
    #[cfg(feature = "real-runtime")]
    use crate::actions::build_import_dialogs::paths::{
        resolve_asset_repo_root, workspace_rules_fixtures_dir,
    };
    use crate::actions::build_import_dialogs::test_support::{
        local_tab_root, preset_names, wanted_affix_ids, wanted_aspect_ids,
    };
    #[cfg(feature = "real-runtime")]
    use crate::actions::build_import_dialogs::test_support::{
        reference_item_rows, reference_item_type,
    };
    use crate::actions::build_import_dialogs::{PROVIDER_D2CORE, PROVIDER_D4BUILDS};
    #[cfg(feature = "real-runtime")]
    use crate::actions::build_import_dialogs::{
        PROVIDER_INFINITY_BUILDS, PROVIDER_MAXROLL, PROVIDER_MOBALYTICS,
    };
    use crate::app_state::AppState;

    // -----------------------------------------------------------------------
    // Descriptor-first import resolution and the hostile-id delete gate.
    // -----------------------------------------------------------------------

    #[test]
    fn import_resolution_uses_the_descriptor_source_path_not_the_first_file() {
        // The source binds strictly to the selected build. Two D2Core
        // fixtures in one directory: `warning_cases.json` sorts first and the
        // inner id `barb-d2-whirlwind` never stem-matches its file
        // `whirlwind_arsenal.json`, so a "first other JSON" resolution would
        // import the WRONG build's JSON.
        let dir = std::env::temp_dir().join("d4lc-import-resolution-d2core");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Resolve the bundled fixture dir deterministically (a
        // nonexistent config root can never shadow it with a real
        // downloaded-builds home).
        let fixture_dir = resolve_builds_dir_with_config_root(
            PROVIDER_D2CORE,
            Path::new("nonexistent-config-root"),
        )
        .expect("bundled d2core fixture dir");
        std::fs::copy(
            fixture_dir.join("warning_cases.json"),
            dir.join("warning_cases.json"),
        )
        .unwrap();
        let selected = dir.join("whirlwind_arsenal.json");
        std::fs::copy(fixture_dir.join("whirlwind_arsenal.json"), &selected).unwrap();

        let mut state = AppState::default_startup();
        let builds = load_builds_for_provider(PROVIDER_D2CORE, &dir).unwrap();
        state
            .build_import
            .provider_builds
            .insert(PROVIDER_D2CORE.to_string(), builds);

        // The descriptor-first resolution imports the build's own JSON.
        let resolved =
            resolve_import_source_path(&state, PROVIDER_D2CORE, "barb-d2-whirlwind", &dir)
                .expect("descriptor path resolves");
        assert_eq!(
            resolved, selected,
            "the Whirlwind row must import its own JSON"
        );

        // The selected source vanishes (renamed or removed cache entry): the
        // import reports it and never substitutes the directory's other file.
        std::fs::remove_file(&selected).unwrap();
        let error = resolve_import_source_path(&state, PROVIDER_D2CORE, "barb-d2-whirlwind", &dir)
            .expect_err("a vanished selected source must fail the import");
        assert!(
            error.contains("whirlwind_arsenal.json") && error.contains("no longer exists"),
            "the error must name the vanished file: {error}"
        );
        assert!(dir.join("warning_cases.json").is_file());

        // Identity resolution alone substitutes nothing either: the D2Core
        // inner id owns no file in this directory, and neither does an id
        // that was never listed.
        assert!(resolve_source_path(PROVIDER_D2CORE, &dir, "barb-d2-whirlwind").is_none());
        assert!(resolve_source_path(PROVIDER_D2CORE, &dir, "maxroll-charm-seal").is_none());
        let error = resolve_import_source_path(&state, PROVIDER_D2CORE, "unlisted-build", &dir)
            .expect_err("an unlisted build has no source");
        assert!(
            error.contains("no stored build JSON for 'unlisted-build'"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unrelated_settings_save_keeps_the_configured_builds_dir() {
        // The panel-projection save (`persist_settings_with_warnings`) is
        // what every settings-screen change runs and the panel state does
        // not own the builds directory. It loads the file first and
        // overlays the panel state, so a change to any other setting must
        // leave the configured directory in place instead of silently
        // resetting it to the resolved default.
        let root = local_tab_root("builds-dir-panel-save");
        let configured = root.join("configured-builds");
        let config = d4lc_config::AppSettings {
            builds_dir: Some(configured.display().to_string()),
            ..d4lc_config::AppSettings::default()
        };
        d4lc_config::save_app_settings(&root, &config).expect("save configured settings");

        let mut state = AppState::default_startup();
        state.shell.settings.type_area_offset_right = 12;
        crate::actions::settings::persist_settings_with_warnings(&mut state, &root);

        let reloaded = d4lc_config::load_app_settings(&root).expect("reload saved settings");
        assert_eq!(
            reloaded.builds_dir, config.builds_dir,
            "an unrelated settings save must not drop the configured builds directory"
        );
        assert_eq!(reloaded.type_area_offset_right, 12);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_app_settings_projects_panel_state_and_build_import_state() {
        let mut state = AppState::default_startup();
        state
            .build_import
            .paragon_toggles
            .set(PROVIDER_D4BUILDS, false);
        let settings = load_app_settings(&state);
        // Default projection: language is the default and
        // multi_builds is empty; the live paragon toggles overlay
        // the projected settings.
        assert_eq!(settings.selected_affix_language, "enUS");
        assert!(settings.rule_settings.multi_builds.is_empty());
        assert!(!settings.is_import_paragon_d4builds_enabled);
        assert!(settings.is_import_paragon_d2core_enabled);
    }

    /// Delete confirm: the removed preset leaves the ComboBox, the first
    /// remaining preset becomes the selection (upstream
    /// `AffixPresetRemovedMessage` handling), and the entry models follow.
    #[test]
    fn delete_confirm_refreshes_the_affix_screen_preset_models() {
        let app = crate::test_window::new_app_window();
        let root = local_tab_root("delete-refresh");
        let mut state = AppState::default_startup();
        // Beta carries the fallback selection's entry; Alpha (deleted) has
        // one too, so the test proves both halves: the deleted preset's
        // entries leave and the fallback's entries project in.
        state
            .affix_preset_editor
            .create_preset("Beta")
            .expect("seed Beta");
        state
            .affix_preset_editor
            .edit_entry(
                "Beta",
                d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "RapidAspect",
                    Some("Gloves"),
                    "#00FF00",
                ),
            )
            .expect("seed Beta entry");
        state
            .affix_preset_editor
            .create_preset("Gamma")
            .expect("seed Gamma");
        state
            .affix_preset_editor
            .create_preset("Alpha")
            .expect("seed Alpha");
        state
            .affix_preset_editor
            .edit_entry(
                "Alpha",
                d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Affix,
                    "AttackSpeed",
                    Some("Amulet"),
                    "#EF4444",
                ),
            )
            .expect("seed Alpha entry");
        assert_eq!(
            state.affix_preset_editor.selected_preset_name.as_deref(),
            Some("Alpha"),
            "precondition: the last created preset is selected"
        );
        app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Alpha"),
            slint::SharedString::from("Beta"),
            slint::SharedString::from("Gamma"),
        ])));
        app.set_selected_preset(slint::SharedString::from("Alpha"));
        app.set_import_selected_preset(slint::SharedString::from("Alpha"));

        handle_delete_confirm(&app, &mut state, &root).expect("the confirmed delete must succeed");

        assert_eq!(
            preset_names(&app),
            vec!["Beta".to_string(), "Gamma".to_string()],
            "the deleted preset must leave the AffixScreen combo"
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            "Beta",
            "the first remaining preset must become the selection"
        );
        assert_eq!(
            app.get_import_selected_preset().to_string(),
            "Beta",
            "the dialog's own preset combo selects the first remaining preset"
        );
        assert!(
            wanted_affix_ids(&app).is_empty(),
            "the deleted preset's entries must leave the entry models: {:?}",
            wanted_affix_ids(&app)
        );
        assert_eq!(
            wanted_aspect_ids(&app),
            vec!["RapidAspect".to_string()],
            "the fallback preset's entries must be projected onto the entry models"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Add-as-preset confirm (`handle_name_done` on a provider tab) with a
    /// real import: the imported preset lands in the AffixScreen combo,
    /// is selected, and its entries project onto the entry models.
    ///
    /// The import itself needs the `real-runtime` feature (the offline
    /// default build reports the feature gap before any preset is
    /// written); the shared refresh it runs is the same one the offline
    /// local-import / merge / delete tests drive.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn add_as_preset_confirm_refreshes_the_affix_screen_preset_models() {
        let app = crate::test_window::new_app_window();
        let root = local_tab_root("add-as-preset-refresh");
        let mut state = AppState::default_startup();
        let builds_dir = workspace_rules_fixtures_dir("maxroll/source");
        state.build_import.builds_dir = Some(builds_dir.clone());
        let build = load_builds_for_provider(PROVIDER_MAXROLL, &builds_dir)
            .expect("the fixture corpus loads")
            .into_iter()
            .find(|build| build.id == "barb-hota-endgame")
            .expect("the bundled Maxroll fixture is listed");
        state.build_import.pending_import = Some(PendingVariantImport {
            provider: PROVIDER_MAXROLL.to_string(),
            build_id: build.id.clone(),
            build_name: build.name.clone(),
            variant_name: "Endgame".to_string(),
        });
        app.set_import_name_text(slint::SharedString::from("Hammer"));
        app.set_import_selected_preset(slint::SharedString::from(""));

        handle_name_done(&app, &mut state, &root)
            .expect("the confirmed import must persist a preset");

        assert_eq!(
            preset_names(&app),
            vec!["Hammer".to_string()],
            "the imported preset must appear in the AffixScreen combo without a restart"
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            "Hammer",
            "the imported preset must be the AffixScreen selection"
        );
        assert_eq!(app.get_import_selected_preset().to_string(), "Hammer");
        let entries = state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == "Hammer")
            .map(|preset| preset.entries.clone())
            .expect("the imported preset is stored");
        assert!(
            !entries.is_empty(),
            "the fixture profile must import at least one entry"
        );
        let mut expected_affix: Vec<String> = entries
            .iter()
            .filter(|entry| entry.kind == d4lc_gui::PresetEntryKind::Affix)
            .map(|entry| entry.id.clone())
            .collect();
        expected_affix.sort();
        // The wanted rail carries one row per localized catalog row: entries
        // that resolve to the same row (the same affix wanted for two slot
        // types) merge into it, so the projected ids are the distinct
        // canonical identities of the imported entries.
        expected_affix.dedup();
        let mut wanted = wanted_affix_ids(&app);
        wanted.sort();
        assert_eq!(
            wanted, expected_affix,
            "the imported preset's affix entries must be projected onto the affix entry model"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One provider leg of the import chain: the stored row to select and how
    /// many source entries the fixture is expected to skip.
    struct ProviderImportLeg {
        provider: &'static str,
        builds_dir: PathBuf,
        build_id: String,
        expected_skips: usize,
        expected_skip_reasons: Vec<String>,
    }

    /// Acceptance: for every provider the whole chain holds — select the
    /// stored build row and its variant, import it, persist it, restart
    /// through the production startup seam, and evaluate a reference item
    /// built from the canonical id/type vocabulary. A preset that stored a
    /// shortened import id, lost a flag or a type on the way through
    /// the editor, or never reached the disk reads as missing
    /// here instead of silently matching nothing. The D4Builds leg uses the
    /// unknown-ids fixture, so the same run also proves the three outcomes
    /// stay apart: this import skips entries and says so, the others report a
    /// fully usable import.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn every_provider_import_survives_a_restart_and_matches_a_reference_item() {
        let app = crate::test_window::new_app_window();
        let root = local_tab_root("provider-import-chain");

        // The Mobalytics import consumes the stored converted entity, so the
        // download-time conversion of the GraphQL fixture runs first (the same
        // seam `convert_and_store_mobalytics_build` uses).
        let mobalytics_dir = root.join("Builds").join(PROVIDER_MOBALYTICS);
        let mobalytics_case = {
            let repo_root = resolve_asset_repo_root().expect("bundled asset data");
            let repo = d4lc_assets::AssetRepository::new(&repo_root);
            let lookups = d4lc_rules::MobalyticsConversionLookups::load(&repo)
                .unwrap_or_else(|error| panic!("{error}"));
            let graphql = std::fs::read_to_string(
                workspace_rules_fixtures_dir("mobalytics/source")
                    .join("build_slugified_rotation.json"),
            )
            .expect("the Mobalytics build fixture");
            let converted = d4lc_rules::convert_mobalytics_graphql_build(
                &graphql,
                "https://mobalytics.gg/diablo-4/builds/slugified-rotation",
                true,
                &lookups,
            )
            .unwrap_or_else(|error| panic!("{error}"))
            .expect("the fixture carries a build document");
            d4lc_rules::export_mobalytics_build_cache(&converted.build, &mobalytics_dir)
                .unwrap_or_else(|error| panic!("{error}"));
            converted.build.id
        };

        let legs = vec![
            ProviderImportLeg {
                provider: PROVIDER_D2CORE,
                builds_dir: workspace_rules_fixtures_dir("d2core/source"),
                build_id: "barb-d2-whirlwind".to_string(),
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
            ProviderImportLeg {
                provider: PROVIDER_D4BUILDS,
                builds_dir: workspace_rules_fixtures_dir("d4builds"),
                build_id: "barb-whirlwind-endgame".to_string(),
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
            ProviderImportLeg {
                provider: PROVIDER_MAXROLL,
                builds_dir: workspace_rules_fixtures_dir("maxroll/source"),
                build_id: "barb-hota-endgame".to_string(),
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
            ProviderImportLeg {
                provider: PROVIDER_INFINITY_BUILDS,
                builds_dir: workspace_rules_fixtures_dir("infinitybuilds/source"),
                build_id: "barb-infinity-whirlwind".to_string(),
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
            ProviderImportLeg {
                provider: PROVIDER_MOBALYTICS,
                builds_dir: mobalytics_dir.clone(),
                build_id: mobalytics_case,
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
            ProviderImportLeg {
                provider: PROVIDER_D4BUILDS,
                builds_dir: workspace_rules_fixtures_dir("d4builds"),
                build_id: "sorc-arc-lash-starter".to_string(),
                // Upstream reports nothing for D4Builds site texts that do
                // not map: the entries are skipped without a Logging line.
                expected_skips: 0,
                expected_skip_reasons: Vec::new(),
            },
        ];

        let mut state = AppState::default_startup();
        for leg in legs {
            let builds = load_builds_for_provider(leg.provider, &leg.builds_dir)
                .unwrap_or_else(|error| panic!("{error}"));
            let row = builds
                .iter()
                .find(|build| build.id == leg.build_id)
                .unwrap_or_else(|| panic!("{} lists '{}'", leg.provider, leg.build_id));
            let build_id = row.id.clone();
            let build_name = row.name.clone();
            let variant_name = row
                .variant_names
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("'{}' lists at least one variant", leg.build_id));
            state.build_import.builds_dir = Some(leg.builds_dir.clone());
            state
                .build_import
                .provider_builds
                .insert(leg.provider.to_string(), builds);
            state.build_import.pending_import = Some(PendingVariantImport {
                provider: leg.provider.to_string(),
                build_id: build_id.clone(),
                build_name,
                variant_name,
            });
            let preset_name = format!("{}-{}", leg.provider, leg.build_id);
            app.set_import_name_text(preset_name.clone().into());

            handle_name_done(&app, &mut state, &root).unwrap_or_else(|error| {
                panic!(
                    "{} import of '{}' failed: {error}",
                    leg.provider, leg.build_id
                )
            });

            // The outcome is visible on the Logging screen: a clean import
            // reports what became usable, a warned import additionally names
            // how many entries were skipped and why.
            let summary = state
                .logging
                .messages
                .iter()
                .rev()
                .map(|message| message.message.clone())
                .find(|message| message.contains(&preset_name))
                .unwrap_or_else(|| {
                    panic!(
                        "the import of '{}' must report its outcome, log: {:?}",
                        leg.build_id,
                        state
                            .logging
                            .messages
                            .iter()
                            .map(|message| message.message.clone())
                            .collect::<Vec<_>>()
                    )
                });
            if leg.build_id == "sorc-arc-lash-starter" {
                for unmapped in ["Imaginary Affix", "Missing Aspect", "Bogus Rune"] {
                    assert!(
                        !state
                            .logging
                            .messages
                            .iter()
                            .any(|message| message.message.contains(unmapped)),
                        "upstream reports no warning for the unmapped D4Builds text '{unmapped}'"
                    );
                }
            }
            if leg.expected_skips == 0 {
                assert!(
                    !summary.contains("Skipped reasons"),
                    "a fully usable import names no skips: {summary}"
                );
            } else {
                assert!(
                    summary.contains(&format!("{} skipped entries", leg.expected_skips)),
                    "the import must report its skip count: {summary}"
                );
                for reason in &leg.expected_skip_reasons {
                    assert!(
                        summary.contains(reason),
                        "the skipped entry '{reason}' must be named: {summary}"
                    );
                }
            }

            // Restart through the production startup seam: the editor starts
            // empty and hydrates from the files the import committed.
            state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(Vec::new());
            state.hydrate_affix_presets_from_config(&root);
            let preset = state
                .affix_preset_editor
                .presets
                .iter()
                .find(|preset| preset.name == preset_name)
                .unwrap_or_else(|| {
                    panic!(
                        "preset '{preset_name}' must survive the restart, got {:?}",
                        state
                            .affix_preset_editor
                            .presets
                            .iter()
                            .map(|preset| preset.name.clone())
                            .collect::<Vec<_>>()
                    )
                });
            assert!(
                !preset.entries.is_empty(),
                "'{preset_name}' must keep the imported entries across the restart"
            );
            assert_eq!(
                state.affix_preset_editor.selected_preset_name.as_deref(),
                Some(preset_name.as_str()),
                "the imported preset stays the persisted selection"
            );

            // The restarted editor is the projection the evaluation reads.
            let projection = state.affix_preset_editor.to_rule_presets();
            let rules_preset = projection
                .affix_presets
                .iter()
                .find(|preset| preset.name == preset_name)
                .expect("the restarted preset must project into the rule presets");
            let item_type = reference_item_type(rules_preset);
            let rows = reference_item_rows(rules_preset, &item_type);
            let item = d4lc_rules::ParsedItemForRules {
                item_type: item_type.clone(),
                rarity: Some("legendary".to_string()),
                affixes: rows.clone(),
            };
            let evaluation =
                d4lc_rules::evaluate_affix_preset(&item, &preset_name, &projection.affix_presets);
            let mut matched_ids: Vec<&str> = evaluation
                .matched_affixes
                .iter()
                .map(|matched| matched.affix_id.as_str())
                .collect();
            matched_ids.sort();
            let mut expected_ids: Vec<&str> =
                rows.iter().map(|row| row.affix_id.as_str()).collect();
            expected_ids.sort();
            assert_eq!(
                matched_ids, expected_ids,
                "every imported affix of the reference item type must match after the restart"
            );
            for matched in &evaluation.matched_affixes {
                let requirement = rules_preset
                    .affixes
                    .iter()
                    .find(|requirement| requirement.affix_id == matched.affix_id)
                    .expect("a match names a preset requirement");
                assert_eq!(
                    matched.color, requirement.color,
                    "the configured import color must survive the restart"
                );
            }
            // The reference item carries one item type, so every requirement of
            // the other types stays missing — and none of its own rows does
            // (the id sets above already proved the matches).
            let expected_missing = rules_preset
                .affixes
                .iter()
                .filter(|requirement| requirement.item_type != item_type)
                .count();
            assert_eq!(
                evaluation.missing_affixes.len(),
                expected_missing,
                "only the requirements of other item types may read as missing: {:?}",
                evaluation.missing_affixes
            );

            // The imported non-affix entries match the same way (id lookup for
            // uniques, id + item type for aspects).
            let non_affix = projection
                .non_affix_presets
                .iter()
                .find(|preset| preset.name == preset_name)
                .expect("the restarted preset must project into the non-affix presets");
            let unique_rows: Vec<d4lc_rules::ParsedNonAffixForRules> = non_affix
                .powers
                .iter()
                .filter(|requirement| requirement.kind == d4lc_rules::RuleNonAffixKind::Unique)
                .map(|requirement| d4lc_rules::ParsedNonAffixForRules {
                    id: requirement.id.clone(),
                    item_type: String::new(),
                    kind: d4lc_rules::RuleNonAffixKind::Unique,
                    is_unique_item: true,
                })
                .collect();
            let verdicts = d4lc_rules::evaluate_non_affix_preset_rows(
                &unique_rows,
                non_affix,
                d4lc_rules::SigilDisplayMode::Whitelisting,
            );
            for (row, verdict) in unique_rows.iter().zip(&verdicts) {
                assert!(
                    matches!(verdict, d4lc_rules::NonAffixRowVerdict::Matched { .. }),
                    "imported unique '{}' must match after the restart, got {verdict:?}",
                    row.id
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The settings panel carries the Infinity paragon flag and writes it
    /// back on every panel-driven save: an import-dialog toggle must not be
    /// reverted by the next unrelated settings save.
    #[test]
    fn infinity_paragon_toggle_survives_a_later_panel_save() {
        let root = local_tab_root("infinity-toggle-panel-save");
        let mut state = AppState::default_startup();
        assert!(
            state
                .shell
                .settings
                .is_import_paragon_infinity_builds_enabled
        );

        apply_paragon_toggle(&mut state, super::PROVIDER_INFINITY_BUILDS, false, &root);
        assert!(
            !state
                .shell
                .settings
                .is_import_paragon_infinity_builds_enabled
        );

        state.shell.settings.overlay_font_size = 22;
        crate::actions::settings::persist_settings_with_warnings(&mut state, &root);

        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert!(!persisted.is_import_paragon_infinity_builds_enabled);
        assert_eq!(persisted.overlay_font_size, 22);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A provider import whose preset write fails must not report success: the
    /// error names the failed save, the editor is restored (no preset exists
    /// only in RAM), and the previously applied revision stays in place — the
    /// old defective order mutated the editor, persisted and closed the
    /// overlay regardless.
    #[cfg(feature = "real-runtime")]
    #[test]
    fn a_failed_provider_import_save_keeps_the_preset_out_of_ram() {
        let app = crate::test_window::new_app_window();
        let root = local_tab_root("provider-import-save-failure");
        let builds_dir = workspace_rules_fixtures_dir("d4builds");
        let mut state = AppState::default_startup();
        let builds = load_builds_for_provider(PROVIDER_D4BUILDS, &builds_dir)
            .expect("the fixture corpus loads");
        let row = builds
            .iter()
            .find(|build| build.id == "barb-whirlwind-endgame")
            .expect("the bundled D4Builds fixture is listed");
        let build_id = row.id.clone();
        let build_name = row.name.clone();
        state.build_import.builds_dir = Some(builds_dir);
        state
            .build_import
            .provider_builds
            .insert(PROVIDER_D4BUILDS.to_string(), builds);
        state.build_import.pending_import = Some(PendingVariantImport {
            provider: PROVIDER_D4BUILDS.to_string(),
            build_id,
            build_name,
            variant_name: "Endgame".to_string(),
        });
        app.set_import_name_text(slint::SharedString::from("Whirlwind"));
        // A committed baseline: the previously applied revision must survive
        // the failed import untouched.
        state
            .affix_preset_editor
            .create_preset("Baseline")
            .expect("seed preset");
        state
            .persist_affix_presets_to_config(&root)
            .expect("seed persist");
        let previous = state
            .applied_config_revision
            .clone()
            .expect("the seed write is an applied revision");
        let presets_target = root.join(d4lc_config::AFFIX_PRESETS_PATH);
        std::fs::remove_file(&presets_target).expect("remove presets file");
        std::fs::create_dir_all(&presets_target).expect("directory in place of the presets file");

        let error = handle_name_done(&app, &mut state, &root)
            .expect_err("a failed save must not report a completed import");

        assert!(
            error.contains("was not saved"),
            "the error must name the failed save: {error}"
        );
        assert_eq!(
            state.applied_config_revision.as_ref(),
            Some(&previous),
            "a failed import keeps the previously applied revision"
        );
        assert!(
            state
                .affix_preset_editor
                .presets
                .iter()
                .all(|preset| preset.name != "Whirlwind"),
            "the editor is restored, so the import exists in RAM nowhere"
        );
        assert!(
            state.build_import.pending_import.is_some(),
            "the failed import keeps its pending selection so it can be retried"
        );
        assert!(
            !state
                .logging
                .messages
                .iter()
                .any(|message| message.message.contains("as preset 'Whirlwind'")),
            "no outcome line may claim an import that never reached the disk"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
