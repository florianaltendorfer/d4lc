//! Multi-build config dialog: projection and the add / remove / colour edits.

use std::path::Path;

use crate::app_state::{AppState, DialogKind, MultiBuildRow};
use crate::slint_generated::AppWindow;

// ---------------------------------------------------------------------------
// Multi-build config dialog projection.
//
// The dialog edits the `multi_builds: Vec<MultiBuildRuleConfig>`
// field of `AppSettings::rule_settings`. We project it to a flat
// Slint-friendly row type so the dialog does not need to know
// about d4lc_config types.
// ---------------------------------------------------------------------------

/// Open the MultiBuildConfig dialog. Seeds the parallel `multi-build-*`
/// root props from `state.build_import.multi_builds` and the add row's
/// preset list from the affix presets.
pub fn handle_open_multi_build_config(app: &AppWindow, state: &mut AppState) {
    project_rows(app, &state.build_import.multi_builds);
    let preset_names: Vec<slint::SharedString> = state
        .affix_preset_editor
        .presets
        .iter()
        .map(|preset| preset.name.clone().into())
        .collect();
    app.set_multi_build_preset_names(slint::ModelRc::new(slint::VecModel::from(preset_names)));
    app.set_multi_build_selected_preset(
        state
            .affix_preset_editor
            .presets
            .first()
            .map(|preset| preset.name.clone())
            .unwrap_or_default()
            .into(),
    );
    app.set_multi_build_color_picker_visible(false);
    refresh_can_add(app, state);
    crate::actions::dialogs::open(app, state, DialogKind::MultiBuildConfig, None);
}

// ---------------------------------------------------------------------------
// Row editing (upstream `MultiBuildConfigViewModel`). Like upstream, every
// add / remove / colour change is saved at once; the dialog only has "Done"
// (`MultiBuildConfigDoneExecute` just closes). The list lives in
// `state.build_import.multi_builds`; the root props are its projection.
// ---------------------------------------------------------------------------

/// Project the list (names, indices, colours and the swatch brushes) onto
/// the root props.
fn project_rows(app: &AppWindow, rows: &[MultiBuildRow]) {
    let names: Vec<slint::SharedString> = rows.iter().map(|r| r.name.clone().into()).collect();
    let indices: Vec<i32> = rows.iter().map(|r| r.index as i32).collect();
    let colors: Vec<slint::SharedString> = rows.iter().map(|r| r.color.clone().into()).collect();
    let brushes: Vec<slint::Color> = rows.iter().map(|r| swatch_color(&r.color)).collect();
    app.set_multi_build_names(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_multi_build_indices(slint::ModelRc::new(slint::VecModel::from(indices)));
    app.set_multi_build_colors(slint::ModelRc::new(slint::VecModel::from(colors)));
    app.set_multi_build_brushes(slint::ModelRc::new(slint::VecModel::from(brushes)));
}

fn swatch_color(value: &str) -> slint::Color {
    let color = d4lc_config::PresetColor::parse(value);
    slint::Color::from_argb_u8(color.a, color.r, color.g, color.b)
}

/// Recompute whether the add button is enabled for the selected preset.
fn refresh_can_add(app: &AppWindow, state: &AppState) {
    let selected = app.get_multi_build_selected_preset().to_string();
    app.set_multi_build_can_add(can_add_build(&state.build_import.multi_builds, &selected));
}

/// Save an edited copy of the list, then project it and drop a stale
/// dialog error.
fn commit_rows(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    rows: Vec<MultiBuildRow>,
) -> Result<(), String> {
    apply_multi_builds(state, rows, config_root)?;
    project_rows(app, &state.build_import.multi_builds);
    refresh_can_add(app, state);
    state.dialog.error.clear();
    app.set_dialog_error(slint::SharedString::default());
    app.set_dialog_error_visible(false);
    Ok(())
}

/// Upstream `CanAddBuildExecute`: a build is addable while no row carries
/// the selected preset's name (`MultiBuildConfigViewModel.cs:93-96`).
fn can_add_build(rows: &[MultiBuildRow], preset: &str) -> bool {
    !preset.is_empty() && !rows.iter().any(|row| row.name == preset)
}

/// Upstream `AddBuildExecute` (`MultiBuildConfigViewModel.cs:98-112`):
/// renumber the existing rows, then append the preset with `Colors.Green`
/// and the next index.
fn add_build(rows: &mut Vec<MultiBuildRow>, preset: &str) -> Result<(), String> {
    if !can_add_build(rows, preset) {
        return Err(format!("'{preset}' is already in the multi-build list"));
    }
    renumber(rows);
    rows.push(MultiBuildRow {
        name: preset.to_string(),
        index: rows.len(),
        color: d4lc_config::PresetColor::GREEN.to_string_form(),
    });
    Ok(())
}

/// Upstream `RemoveBuildExecute` (`:125-143`): drop the row and renumber.
fn remove_build(rows: &mut Vec<MultiBuildRow>, index: usize) -> Result<(), String> {
    if index >= rows.len() {
        return Err(format!("no multi-build row {index}"));
    }
    rows.remove(index);
    renumber(rows);
    Ok(())
}

/// Upstream `SetColorBuildExecute` (`:145-163`) writes the picker's colour
/// to the row. The colour is parsed strictly and stored in canonical form.
fn set_build_color(rows: &mut [MultiBuildRow], index: usize, color: &str) -> Result<(), String> {
    let parsed = d4lc_config::PresetColor::try_parse(color)?;
    let row = rows
        .get_mut(index)
        .ok_or_else(|| format!("no multi-build row {index}"))?;
    row.color = parsed.to_string_form();
    Ok(())
}

fn renumber(rows: &mut [MultiBuildRow]) {
    for (index, row) in rows.iter_mut().enumerate() {
        row.index = index;
    }
}

/// The add row's ComboBox selection changed.
pub fn handle_multi_build_preset_selected(app: &AppWindow, state: &AppState, preset: &str) {
    app.set_multi_build_selected_preset(preset.into());
    refresh_can_add(app, state);
}

/// The add row's "+" click: append the selected preset and save.
pub fn handle_multi_build_add(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let mut rows = state.build_import.multi_builds.clone();
    add_build(&mut rows, &app.get_multi_build_selected_preset())?;
    commit_rows(app, state, config_root, rows)
}

/// A row's remove click: drop the row, renumber and save.
pub fn handle_multi_build_remove(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    index: i32,
) -> Result<(), String> {
    let mut rows = state.build_import.multi_builds.clone();
    remove_build(&mut rows, usize::try_from(index).unwrap_or(usize::MAX))?;
    commit_rows(app, state, config_root, rows)
}

/// A row's colour label click: open the embedded picker on that row's
/// colour.
pub fn handle_multi_build_color_clicked(
    app: &AppWindow,
    state: &AppState,
    index: i32,
) -> Result<(), String> {
    let row = usize::try_from(index)
        .ok()
        .and_then(|index| state.build_import.multi_builds.get(index))
        .ok_or_else(|| format!("no multi-build row {index}"))?;
    let current = d4lc_config::PresetColor::parse(&row.color).to_string_form();
    app.set_multi_build_color_picker_row(index);
    app.set_multi_build_color_picker_value(current.into());
    app.set_multi_build_color_picker_visible(true);
    Ok(())
}

/// Picker confirm: write the colour to the clicked row only and save.
pub fn handle_multi_build_color_picker_done(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
) -> Result<(), String> {
    let mut rows = state.build_import.multi_builds.clone();
    let index = usize::try_from(app.get_multi_build_color_picker_row()).unwrap_or(usize::MAX);
    set_build_color(&mut rows, index, &app.get_multi_build_color_picker_value())?;
    commit_rows(app, state, config_root, rows)?;
    app.set_multi_build_color_picker_visible(false);
    Ok(())
}

/// Picker cancel: close without writing back.
pub fn handle_multi_build_color_picker_cancel(app: &AppWindow) {
    app.set_multi_build_color_picker_visible(false);
}

/// Validate the build list, persist it to `rule_settings.multi_builds`, and
/// apply it to `AppState`.
///
/// Upstream's add / remove / colour commands only rewrite `MultiBuildList`
/// (`MultiBuildConfigViewModel.cs:114-115,138-139`); they never touch
/// `IsMultiBuildModeEnabled`, which stays the toggle of the affix, aspect,
/// sigil, unique and rune dialogs. The list is saved before it is applied,
/// so a failed save leaves both the file and the state unchanged.
fn apply_multi_builds(
    state: &mut AppState,
    builds: Vec<crate::app_state::MultiBuildRow>,
    config_root: &Path,
) -> Result<(), String> {
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cleaned = Vec::with_capacity(builds.len());
    for (i, build) in builds.into_iter().enumerate() {
        let name = build.name.trim().to_string();
        if name.is_empty() {
            return Err(format!("multi-build row {i}: name must not be empty"));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!("duplicate multi-build name '{name}'"));
        }
        let color = build.color.trim().to_string();
        if color.is_empty() {
            return Err(format!(
                "multi-build row {i} ('{name}'): color must not be empty"
            ));
        }
        cleaned.push(crate::app_state::MultiBuildRow {
            name,
            index: build.index,
            color,
        });
    }

    let mut config = d4lc_config::load_app_settings(config_root)
        .map_err(|error| format!("Failed to load app settings: {error}"))?;
    config.rule_settings.multi_builds = cleaned
        .iter()
        .map(|row| d4lc_config::MultiBuildRuleConfig {
            name: row.name.clone(),
            index: row.index,
            color: row.color.clone(),
        })
        .collect();
    d4lc_config::save_app_settings(config_root, &config)
        .map_err(|error| format!("Failed to save app settings: {error}"))?;

    state.build_import.multi_builds.clear();
    state.build_import.multi_builds.extend(cleaned.clone());
    state.bump_rules_input_revision();
    validate_multi_build_names(state, &cleaned);
    Ok(())
}

/// Upstream `ValidateMultiBuild` (`AffixManager.cs:1106-1120`) sends an
/// error for every configured build whose name matches no preset —
/// upstream `SetMultiBuildMode` would silently draw nothing for it. A
/// build's `Name` IS the preset name it evaluates against, so the miss
/// is logged on the Logging screen, the d4lc analog of upstream's
/// `ErrorOccurredMessage`.
fn validate_multi_build_names(state: &mut AppState, builds: &[crate::app_state::MultiBuildRow]) {
    for build in builds {
        if !state
            .affix_preset_editor
            .presets
            .iter()
            .any(|preset| preset.name == build.name)
        {
            state.append_log_message(
                format!("multi build #{} not found: {}", build.index + 1, build.name),
                crate::clock::hh_mm_now(),
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state::{AppState, MultiBuildRow};
    use slint::Model;

    fn row(name: &str, index: usize, color: &str) -> MultiBuildRow {
        MultiBuildRow {
            name: name.to_string(),
            index,
            color: color.to_string(),
        }
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn add_appends_green_with_the_next_index_and_refuses_duplicates() {
        // MultiBuildConfigViewModel.cs:93-112.
        let mut rows = vec![row("Speed", 7, "#CC3333")];
        assert!(can_add_build(&rows, "Boss"));
        assert!(!can_add_build(&rows, "Speed"));
        assert!(!can_add_build(&rows, ""));

        add_build(&mut rows, "Boss").unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].index, 0, "existing rows are renumbered first");
        assert_eq!(rows[1], row("Boss", 1, "#008000"));
        assert!(add_build(&mut rows, "Boss").is_err());
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn remove_drops_the_row_and_renumbers() {
        // MultiBuildConfigViewModel.cs:125-143.
        let mut rows = vec![
            row("A", 0, "Green"),
            row("B", 1, "Red"),
            row("C", 2, "Blue"),
        ];
        remove_build(&mut rows, 1).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.index))
                .collect::<Vec<_>>(),
            vec![("A", 0), ("C", 1)]
        );
        assert!(remove_build(&mut rows, 5).is_err());
    }

    #[test]
    fn set_color_stores_the_canonical_form_and_rejects_garbage() {
        let mut rows = vec![row("A", 0, "Green"), row("B", 1, "Red")];
        set_build_color(&mut rows, 1, "#3b82f6").unwrap();
        assert_eq!(rows[1].color, "#3B82F6");
        assert_eq!(rows[0].color, "Green", "only the clicked row changes");
        assert!(set_build_color(&mut rows, 1, "not a color").is_err());
        assert_eq!(rows[1].color, "#3B82F6");
        assert!(set_build_color(&mut rows, 9, "#FFFFFF").is_err());
    }

    #[test]
    fn dialog_edits_are_saved_at_once() {
        // Upstream saves on add / remove / colour (MultiBuildConfigViewModel.cs
        // :114-115,138-139,162); the dialog only has "Done".
        let app = crate::test_window::new_app_window();
        let root = temp_root("d4lc-test-multi-build-dialog");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        state.affix_preset_editor.create_preset("Boss").unwrap();
        let persisted = |root: &std::path::Path| {
            d4lc_config::load_app_settings(root)
                .unwrap()
                .rule_settings
                .multi_builds
        };

        handle_open_multi_build_config(&app, &mut state);
        assert_eq!(app.get_multi_build_preset_names().row_count(), 2);
        assert!(app.get_multi_build_can_add());

        // The add row starts on the first preset in the list.
        let first = app.get_multi_build_selected_preset().to_string();
        let other = if first == "Speed" { "Boss" } else { "Speed" };
        handle_multi_build_add(&app, &mut state, &root).unwrap();
        assert_eq!(app.get_multi_build_names().row_count(), 1);
        assert_eq!(app.get_multi_build_brushes().row_count(), 1);
        assert_eq!(persisted(&root).len(), 1, "add is saved at once");
        assert!(
            !app.get_multi_build_can_add(),
            "the preset is in the rotation now"
        );
        handle_multi_build_preset_selected(&app, &state, other);
        handle_multi_build_add(&app, &mut state, &root).unwrap();
        assert_eq!(persisted(&root).len(), 2);

        // A failed edit shows an error, a later success clears it.
        app.set_dialog_error("stale".into());
        app.set_dialog_error_visible(true);

        handle_multi_build_color_clicked(&app, &state, 1).unwrap();
        assert!(app.get_multi_build_color_picker_visible());
        app.set_multi_build_color_picker_value("#3B82F6".into());
        handle_multi_build_color_picker_done(&app, &mut state, &root).unwrap();
        assert!(!app.get_multi_build_color_picker_visible());
        assert!(!app.get_dialog_error_visible(), "success clears the error");
        assert_eq!(persisted(&root)[1].color, "#3B82F6");

        handle_multi_build_remove(&app, &mut state, &root, 0).unwrap();
        assert_eq!(app.get_multi_build_names().row_count(), 1);
        assert_eq!(app.get_multi_build_indices().row_data(0), Some(0));
        let saved = persisted(&root);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, other);
        assert_eq!(saved[0].color, "#3B82F6");
        assert_eq!(state.build_import.multi_builds.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn confirmed_multi_builds_persist_and_survive_a_restart() {
        let root = temp_root("d4lc-test-multi-build-persist");
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        assert!(!state.shell.settings.multi_build_mode_enabled);

        apply_multi_builds(
            &mut state,
            vec![row(" Speed ", 0, "#CC3333"), row("Boss", 1, "Green")],
            &root,
        )
        .unwrap();

        assert_eq!(state.build_import.multi_builds.len(), 2);
        assert_eq!(state.build_import.multi_builds[0].name, "Speed");
        assert!(
            !state.shell.settings.multi_build_mode_enabled,
            "upstream's confirm never flips IsMultiBuildModeEnabled"
        );
        let persisted = d4lc_config::load_app_settings(&root).unwrap();
        assert_eq!(persisted.rule_settings.multi_builds.len(), 2);
        assert_eq!(persisted.rule_settings.multi_builds[1].name, "Boss");
        assert_eq!(persisted.rule_settings.multi_builds[1].color, "#008000");
        assert!(!persisted.rule_settings.multi_build_enabled);

        // The startup hydration path reads the same list back.
        let mut restarted = AppState::default_startup();
        restarted.hydrate_build_import_from_settings(&persisted);
        assert_eq!(restarted.build_import.multi_builds.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_multi_builds_change_neither_state_nor_file() {
        let root = temp_root("d4lc-test-multi-build-invalid");
        let mut state = AppState::default_startup();
        for bad in [
            vec![row("", 0, "Green")],
            vec![row("Speed", 0, "Green"), row("Speed", 1, "Red")],
            vec![row("Speed", 0, " ")],
        ] {
            assert!(apply_multi_builds(&mut state, bad, &root).is_err());
        }
        assert!(state.build_import.multi_builds.is_empty());
        assert!(!root.join("Config").join("Settings.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn multi_build_names_are_validated_against_existing_presets() {
        // Upstream `ValidateMultiBuild`: a configured build whose name
        // matches no preset is warned about, because that build would
        // silently draw nothing.
        let mut state = AppState::default_startup();
        state.affix_preset_editor.create_preset("Speed").unwrap();
        let builds = vec![
            crate::app_state::MultiBuildRow {
                name: "Speed".to_string(),
                index: 0,
                color: "#CC3333".to_string(),
            },
            crate::app_state::MultiBuildRow {
                name: "Ghost Preset".to_string(),
                index: 1,
                color: "#22C55E".to_string(),
            },
        ];

        validate_multi_build_names(&mut state, &builds);

        assert!(
            state.logging.messages.iter().any(|message| message
                .message
                .contains("multi build #2 not found: Ghost Preset")),
            "a configured build without a preset must be logged"
        );
        assert!(
            !state
                .logging
                .messages
                .iter()
                .any(|message| message.message.contains("#1")),
            "a build whose name matches a preset draws no warning"
        );
    }
}
