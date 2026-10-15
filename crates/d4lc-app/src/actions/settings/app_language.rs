//! The UI-language selector: the sidebar combo of the settings screen
//! (`SettingsView.xaml:58-73`), its seed (`SettingsViewModel.cs:417-421`) and
//! the selection handler (`SettingsViewModel.cs:222-235`).
//!
//! The UI language is separate from the game-data language (`SelectedAffixLanguage`),
//! which the affix screen's combo keeps selecting.

use std::path::Path;
use std::rc::Rc;

use d4lc_config::load_app_settings;
use d4lc_gui::settings::{app_language_by_id, app_language_by_name, AppLanguage, APP_LANGUAGES};
use slint::{SharedString, VecModel};

use super::panel::{persist_settings_with_warnings, replacement_options};
use crate::actions::debug;
use crate::app_state::AppState;
use crate::i18n;
use crate::slint_generated::AppWindow;

/// Seed the selector from the persisted `SelectedAppLanguage` and apply that
/// language.
///
/// Slint picks a bundled language from the system locale when the first window
/// is created, so the stored choice is always applied explicitly: the UI
/// language follows the setting, not Slint's own locale pick. A stored culture
/// the selector does not offer selects nothing in the combo and is left in the
/// file, as upstream leaves its selection null; the UI then follows the desktop
/// locale where a resource file applies and is English otherwise, with a
/// warning naming the stored value.
///
/// Runs on the UI thread after the window exists. Both the real launch path
/// and the headless harness composition reach it through
/// `apply_settings_language_projection`.
pub fn apply_app_language_projection(app: &AppWindow, config_root: &Path) {
    let names: Vec<SharedString> = APP_LANGUAGES
        .iter()
        .map(|language| SharedString::from(language.name))
        .collect();
    if let Some(model) = replacement_options(&app.get_app_language_options(), names) {
        app.set_app_language_options(model);
    }

    let settings = load_app_settings(config_root).unwrap_or_default();
    let selection = app_language_by_id(&settings.selected_app_language);
    app.set_selected_app_language(SharedString::from(
        selection.map_or("", |language| language.name),
    ));
    let system = sys_locale::get_locale();
    let (culture, fell_back) =
        i18n::resolve_culture(&settings.selected_app_language, system.as_deref());
    if fell_back {
        eprintln!(
            "[d4lc] warning: stored UI language '{}' is not offered; using {culture} (system locale {})",
            settings.selected_app_language,
            system.as_deref().unwrap_or("unknown")
        );
    }
    if let Err(error) = i18n::select(culture) {
        eprintln!("[d4lc] UI language '{culture}' not applied: {error}");
    }
}

/// Project the tab captions in the current UI language. The tab model is Rust
/// text (`map_tab_label`), so a language switch and the startup seed both
/// project it again.
pub(crate) fn project_tab_labels(app: &AppWindow, state: &AppState) {
    let labels: Vec<SharedString> = state
        .shell
        .tabs
        .iter()
        .map(crate::map_tab_label)
        .map(SharedString::from)
        .collect();
    app.set_tab_labels(Rc::new(VecModel::from(labels)).into());
}

/// Handle the UI-language selection from the settings combo.
///
/// Switches the translation, persists the culture id and re-projects the text
/// Rust owns (the tab captions and the Debug screen's OCR panel titles).
/// `@tr` text re-evaluates by itself.
///
/// # Errors
///
/// Returns an error for a caption outside the selector's model, or when the
/// translation cannot be switched; the setting is left unchanged then.
pub fn handle_app_language_selected(
    app: &AppWindow,
    state: &mut AppState,
    config_root: &Path,
    caption: &str,
) -> Result<(), String> {
    let language = app_language_by_name(caption.trim()).ok_or_else(|| {
        format!("unknown UI language '{caption}'; the selector offers only the upstream languages")
    })?;
    i18n::select(language.id).map_err(|error| error.to_string())?;
    remember_app_language(state, config_root, language);
    app.set_selected_app_language(language.name.into());
    project_tab_labels(app, state);
    debug::project_debug_panels(app, state, &debug::screenshot::default_screenshot_dir());
    debug::project_live_debug_stages(app, state);
    Ok(())
}

/// Record the choice in the panel state and `Config/Settings.json`. An
/// unchanged selection must not touch the file: the hot-reload monitor
/// compares file stamps.
fn remember_app_language(state: &mut AppState, config_root: &Path, language: &AppLanguage) {
    if state.shell.settings.set_app_language(language.id) {
        state.bump_rules_input_revision();
        persist_settings_with_warnings(state, config_root);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use d4lc_gui::settings::app_language_by_id;
    use slint::Model;

    use super::*;
    use crate::actions::settings::test_support::with_temp_config_root;

    fn persisted(config_root: &Path) -> d4lc_config::AppSettings {
        load_app_settings(config_root).expect("settings load")
    }

    #[test]
    fn a_choice_is_stored_apart_from_the_game_data_language() {
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            let french = app_language_by_id("fr-FR").expect("offered");
            remember_app_language(&mut state, config_root, french);
            let stored = persisted(config_root);
            assert_eq!(stored.selected_app_language, "fr-FR");
            assert_eq!(stored.selected_affix_language, "enUS");
            assert_eq!(state.shell.settings.app_language, "fr-FR");
        });
    }

    #[test]
    fn a_changed_choice_bumps_the_rules_input_revision_and_an_unchanged_one_does_not() {
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            let before = state.rules_input_revision;
            let english = app_language_by_id("en-US").expect("offered");
            remember_app_language(&mut state, config_root, english);
            assert_eq!(state.rules_input_revision, before);

            let french = app_language_by_id("fr-FR").expect("offered");
            remember_app_language(&mut state, config_root, french);
            assert!(state.rules_input_revision > before);
        });
    }

    #[test]
    fn an_unchanged_choice_does_not_rewrite_the_file() {
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            let english = app_language_by_id("en-US").expect("offered");
            remember_app_language(&mut state, config_root, english);
            assert!(
                !config_root.join(d4lc_config::SETTINGS_PATH).exists(),
                "selecting the language that is already active must not create or touch the file"
            );
        });
    }

    #[test]
    fn the_selector_projects_captions_and_applies_the_stored_language() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut stored = d4lc_config::AppSettings {
                selected_app_language: "zh-CN".to_string(),
                ..Default::default()
            };
            d4lc_config::save_app_settings(config_root, &stored).expect("settings save");

            apply_app_language_projection(&app, config_root);
            let captions: Vec<String> = (0..app.get_app_language_options().row_count())
                .filter_map(|row| app.get_app_language_options().row_data(row))
                .map(|caption| caption.to_string())
                .collect();
            assert_eq!(
                captions,
                [
                    "German",
                    "English",
                    "Spanish (EU)",
                    "French",
                    "Chinese (Simplified)"
                ]
            );
            assert_eq!(app.get_selected_app_language(), "Chinese (Simplified)");

            // A stored culture the selector does not offer selects nothing and
            // is left in the file.
            stored.selected_app_language = "es-MX".to_string();
            d4lc_config::save_app_settings(config_root, &stored).expect("settings save");
            apply_app_language_projection(&app, config_root);
            assert_eq!(app.get_selected_app_language(), "");
            assert_eq!(persisted(config_root).selected_app_language, "es-MX");
        });
    }

    #[test]
    fn a_pick_switches_persists_and_refuses_unknown_captions() {
        let app = crate::test_window::new_app_window();
        with_temp_config_root(|config_root| {
            let mut state = AppState::default_startup();
            apply_app_language_projection(&app, config_root);

            handle_app_language_selected(&app, &mut state, config_root, "German")
                .expect("German is offered");
            assert_eq!(app.get_selected_app_language(), "German");
            assert_eq!(persisted(config_root).selected_app_language, "de-DE");

            // The selector can only produce its own captions; a culture id
            // or a language upstream does not offer is a programmatic caller.
            for caption in ["de-DE", "Klingon", "Spanish (LA)"] {
                assert!(
                    handle_app_language_selected(&app, &mut state, config_root, caption).is_err(),
                    "{caption} must be refused"
                );
            }
            assert_eq!(app.get_selected_app_language(), "German");
            assert_eq!(persisted(config_root).selected_app_language, "de-DE");
        });
    }
}
