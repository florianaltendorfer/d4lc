//! Merge tab (upstream tab 1): the merge rows, color picker and merge.

use std::path::Path;

use super::add_preset::name_exists;
use super::dialog::seed_preset_models;
use crate::app_state::{AppState, PendingMerge};
use crate::slint_generated::AppWindow;

/// The default color of the Merge tab's rows (upstream constructor seeds
/// `ColorBuild1/2/12` from `Settings.DefaultColorNormal`; the port's
/// compiled default normal color is Green — see
/// `actions::affix_cmds::default_entry_color`).
pub const DEFAULT_MERGE_COLOR: &str = "#008000";

// ---------------------------------------------------------------------------
// Merge tab (upstream tab 1, `MergeBuildsExecute` +
// `SetAffixColorBuild{N}Execute`): the Build 1 / Build 2 / Build 1+2 rows.
// ---------------------------------------------------------------------------

/// The Build 1+2 row's swatch click (upstream `SetAffixColorBuild{N}
/// Execute`, gated by the row's change-color checkbox): open the embedded
/// color picker seeded with that row's OWN color. Upstream seeds the
/// Build 2 dialog with `ColorBuild1` — an upstream bug the port
/// deliberately does not copy.
pub fn handle_merge_color_clicked(app: &AppWindow, row: i32) -> Result<(), String> {
    let (enabled, current) = match row {
        0 => (
            app.get_import_merge_change_color1(),
            app.get_import_merge_color1().to_string(),
        ),
        1 => (
            app.get_import_merge_change_color2(),
            app.get_import_merge_color2().to_string(),
        ),
        2 => (
            app.get_import_merge_change_color12(),
            app.get_import_merge_color12().to_string(),
        ),
        other => return Err(format!("no merge color row {other}")),
    };
    if !enabled {
        return Err(format!(
            "enable the row's change-color checkbox first (row {row})"
        ));
    }
    app.set_import_color_picker_row(row);
    app.set_import_color_picker_value(current.into());
    app.set_import_color_picker_visible(true);
    Ok(())
}

/// Color-picker confirm: validate the hex value and write it back to the
/// clicked row only (upstream writes the picker's color unconditionally;
/// the port applies the same non-empty / ≤16-char validation the
/// SetAffixColor dialog flow uses).
pub fn handle_color_picker_done(app: &AppWindow) -> Result<(), String> {
    let color = app.get_import_color_picker_value().trim().to_string();
    if color.is_empty() {
        return Err("Color is required.".to_string());
    }
    if color.chars().count() > 16 {
        return Err("Color must be 16 characters or fewer.".to_string());
    }
    match app.get_import_color_picker_row() {
        0 => app.set_import_merge_color1(color.into()),
        1 => app.set_import_merge_color2(color.into()),
        2 => app.set_import_merge_color12(color.into()),
        other => return Err(format!("no merge color row {other}")),
    }
    app.set_import_color_picker_visible(false);
    Ok(())
}

/// Color-picker cancel: close without writing back.
pub fn handle_color_picker_cancel(app: &AppWindow) {
    app.set_import_color_picker_visible(false);
}

/// Merge button click (upstream `MergeBuildsExecute` opening the Confirm
/// name dialog): capture the two selections and seed the shared
/// Confirm-name overlay with Build 1's name and exactly the two
/// suggestions `[Build1.Name, Build2.Name]`.
pub fn handle_merge_clicked(app: &AppWindow, state: &mut AppState) -> Result<(), String> {
    let build1_name = app.get_import_merge_build1().trim().to_string();
    let build2_name = app.get_import_merge_build2().trim().to_string();
    if build1_name.is_empty() || build2_name.is_empty() {
        return Err("select both a Build 1 and a Build 2 preset first".to_string());
    }
    // Both presets must exist (the combos are seeded from the editor, but
    // the selection may go stale across an in-dialog removal).
    find_preset(state, &build1_name)?;
    find_preset(state, &build2_name)?;

    state.build_import.pending_import = None;
    state.bump_rules_input_revision();
    state.build_import.pending_merge = Some(PendingMerge);
    state.bump_rules_input_revision();
    let suggestions: Vec<slint::SharedString> =
        vec![build1_name.clone().into(), build2_name.into()];
    app.set_import_name_suggestions(slint::ModelRc::new(slint::VecModel::from(suggestions)));
    // Initial name = Build 1's name; it already exists (it IS a preset),
    // so the red overwrite warning shows exactly like upstream.
    app.set_import_name_text(build1_name.clone().into());
    app.set_import_name_overwrite_warning(name_exists(state, &build1_name));
    app.set_import_name_visible(true);
    Ok(())
}

/// Confirm-name Done for the merge flow: run the pure merge, silently
/// overwrite a same-name preset, persist, and re-select the merged
/// preset — the AffixScreen preset ComboBox and entry models refresh in
/// the same session through the shared preset-model seam.
pub(super) fn handle_merge_name_done(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let requested_name = app.get_import_name_text().to_string();
    let build1 = find_preset(state, app.get_import_merge_build1().as_str())?;
    let build2 = find_preset(state, app.get_import_merge_build2().as_str())?;
    let options = d4lc_gui::MergeColorOptions {
        change_color_build1: app.get_import_merge_change_color1(),
        color_build1: app.get_import_merge_color1().to_string(),
        change_color_build2: app.get_import_merge_change_color2(),
        color_build2: app.get_import_merge_color2().to_string(),
        change_color_build12: app.get_import_merge_change_color12(),
        color_build12: app.get_import_merge_color12().to_string(),
    };
    let merged = d4lc_gui::merge_affix_presets(&build1, &build2, &requested_name, &options);
    let merged_name = merged.name.clone();

    // The editor's `import_preset` silently removes the same-name preset
    // first, sorts, and selects (upstream `RemoveAll` + `AddAffixPreset`).
    // A merged preset carries no paragon boards (upstream constructs a
    // fresh `AffixPreset`), so any stale routes under the overwritten
    // name are dropped with it. The editor is snapshotted so a failed
    // commit restores it: the merge must not leave a preset that only
    // exists in RAM.
    let editor_before = state.affix_preset_editor.clone();
    state.affix_preset_editor.import_preset(merged, Vec::new());
    state
        .affix_preset_editor
        .set_paragon_routes(&merged_name, Vec::new());
    // Merge order is RAM change → persist → success: the merged preset
    // and the new selection are committed before the overlay reports
    // success, and a failed write is returned so the error stays
    // visible instead of closing on a merge that was never stored.
    if let Err(error) = state.persist_affix_presets_and_selection_to_config(config_root) {
        state.affix_preset_editor = editor_before;
        state.bump_rules_input_revision();
        return Err(format!(
            "merged preset '{merged_name}' was not saved: {error}"
        ));
    }
    // The merge is a preset-list mutation: refresh the AffixScreen preset
    // ComboBox and entry models, then the dialog's own preset combo.
    crate::actions::affix::refresh_affix_screen_preset_models(app, state);
    seed_preset_models(app, state);
    app.set_import_selected_preset(merged_name.clone().into());
    state.build_import.pending_merge = None;
    state.bump_rules_input_revision();
    app.set_import_name_visible(false);

    eprintln!(
        "[d4lc] merged \"{}\" + \"{}\" into preset \"{merged_name}\" ({} entries)",
        build1.name,
        build2.name,
        state
            .affix_preset_editor
            .presets
            .iter()
            .find(|preset| preset.name == merged_name)
            .map(|preset| preset.entries.len())
            .unwrap_or(0),
    );
    Ok(())
}

/// The editor preset for `name`, cloned for the merge flow.
fn find_preset(state: &AppState, name: &str) -> Result<d4lc_gui::AffixPresetEditorPreset, String> {
    state
        .affix_preset_editor
        .presets
        .iter()
        .find(|preset| preset.name == name)
        .cloned()
        .ok_or_else(|| format!("preset '{name}' not found"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::build_import_dialogs::add_preset::handle_name_done;
    use crate::actions::build_import_dialogs::test_support::{
        local_tab_root, preset_names, wanted_aspect_ids,
    };
    use crate::app_state::AppState;

    /// Merge confirm (`handle_name_done` on the Merge tab): the merged
    /// preset appears in the ComboBox, becomes the selection, and its
    /// merged entries project onto the entry models.
    #[test]
    fn merge_confirm_refreshes_the_affix_screen_preset_models() {
        let app = crate::test_window::new_app_window();
        let root = local_tab_root("merge-refresh");
        let mut state = AppState::default_startup();
        state
            .affix_preset_editor
            .create_preset("Speed")
            .expect("seed Speed");
        state
            .affix_preset_editor
            .edit_entry(
                "Speed",
                d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "RapidAspect",
                    Some("Gloves"),
                    "#00FF00",
                ),
            )
            .expect("seed Speed entry");
        state
            .affix_preset_editor
            .create_preset("Boss")
            .expect("seed Boss");
        state
            .affix_preset_editor
            .edit_entry(
                "Boss",
                d4lc_gui::PresetEntry::non_affix(
                    d4lc_gui::PresetEntryKind::Aspect,
                    "GhostWalker",
                    Some("Boots"),
                    "#FF0000",
                ),
            )
            .expect("seed Boss entry");
        app.set_preset_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("Boss"),
            slint::SharedString::from("Speed"),
        ])));
        app.set_selected_preset(slint::SharedString::from("Boss"));
        // The Merge tab's two selections and the Confirm-name box.
        app.set_import_merge_build1(slint::SharedString::from("Speed"));
        app.set_import_merge_build2(slint::SharedString::from("Boss"));
        app.set_import_name_text(slint::SharedString::from("Merged"));
        state.build_import.pending_merge = Some(PendingMerge);

        handle_name_done(&app, &mut state, &root).expect("the merge confirm must succeed");

        assert_eq!(
            preset_names(&app),
            vec![
                "Boss".to_string(),
                "Merged".to_string(),
                "Speed".to_string()
            ],
            "the merged preset must appear in the AffixScreen combo without a restart"
        );
        assert_eq!(
            app.get_selected_preset().to_string(),
            "Merged",
            "the merged preset must be the AffixScreen selection"
        );
        assert_eq!(
            app.get_import_selected_preset().to_string(),
            "Merged",
            "the dialog's own preset combo re-selects the merge"
        );
        let mut wanted = wanted_aspect_ids(&app);
        wanted.sort();
        assert_eq!(
            wanted,
            vec!["GhostWalker".to_string(), "RapidAspect".to_string()],
            "the merged preset's entries must be projected onto the entry models"
        );
        assert!(state.build_import.pending_merge.is_none());

        // The merged preset must also be usable: a restart restores it from
        // the preset file, and a reference item carrying its merged aspects
        // matches them (the merge's color options ride along).
        state.affix_preset_editor = d4lc_gui::AffixPresetEditorState::new(Vec::new());
        state.hydrate_affix_presets_from_config(&root);
        let projection = state.affix_preset_editor.to_rule_presets();
        let non_affix = projection
            .non_affix_presets
            .iter()
            .find(|preset| preset.name == "Merged")
            .expect("the merged preset must survive a restart");
        let rows: Vec<d4lc_rules::ParsedNonAffixForRules> = non_affix
            .powers
            .iter()
            .map(|requirement| d4lc_rules::ParsedNonAffixForRules {
                id: requirement.id.clone(),
                item_type: requirement.item_type.clone().unwrap_or_default(),
                kind: requirement.kind,
                is_unique_item: false,
            })
            .collect();
        let verdicts = d4lc_rules::evaluate_non_affix_preset_rows(
            &rows,
            non_affix,
            d4lc_rules::SigilDisplayMode::Whitelisting,
        );
        assert!(
            rows.len() == 2
                && verdicts.iter().all(|verdict| matches!(
                    verdict,
                    d4lc_rules::NonAffixRowVerdict::Matched { .. }
                )),
            "both merged aspects must match after a restart: {verdicts:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
