//! Shared fixtures of the build-import dialog tests.

use crate::slint_generated::AppWindow;
use slint::Model;

pub(super) fn local_tab_root(test_name: &str) -> crate::test_root::TestRoot {
    crate::test_root::TestRoot::new(&format!("app-{test_name}"))
}

pub(super) fn write_import_file(
    root: &std::path::Path,
    name: &str,
    text: &str,
) -> std::path::PathBuf {
    let path = root.join(name);
    std::fs::write(&path, text).expect("write import file");
    path
}

// -----------------------------------------------------------------------
// AffixScreen preset-model refresh after a preset mutation
//
// Upstream `AffixViewModel` exposes `AffixPresets` and
// `SelectedAffixPreset` as observable state, so every preset-list
// mutation updates the AffixScreen preset ComboBox and its bound
// wanted-entry lists in the same session. The Slint root properties are
// static, so each mutation path must call the shared refresh
// (`actions::affix::refresh_affix_screen_preset_models`); these tests
// drive the real confirm handlers and read the root models back.
// -----------------------------------------------------------------------

/// The root `preset-names` model (the AffixScreen preset ComboBox).
pub(super) fn preset_names(app: &AppWindow) -> Vec<String> {
    let model = app.get_preset_names();
    (0..model.row_count())
        .map(|row| {
            model
                .row_data(row)
                .map(|name| name.to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// The wanted-row ids of the projected affix entry model.
pub(super) fn wanted_affix_ids(app: &AppWindow) -> Vec<String> {
    let model = app.get_affix_entries();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .filter(|entry| entry.is_wanted)
        .map(|entry| entry.id.to_string())
        .collect()
}

/// The wanted-row ids of the projected aspect entry model.
pub(super) fn wanted_aspect_ids(app: &AppWindow) -> Vec<String> {
    let model = app.get_aspect_entries();
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .filter(|entry| entry.is_wanted)
        .map(|entry| entry.id.to_string())
        .collect()
}

/// Serialises the tests that swap the process-wide file-picker seam
/// (`set_import_file_dialog`): two parallel tests would otherwise be
/// able to observe each other's mock.
pub(super) static IMPORT_FILE_DIALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The item type of a preset that owns distinct affix ids: each tooltip
/// row then maps to exactly one preset entry, so the reference item has
/// one row per requirement (ties resolve to the type with the most rows).
pub(super) fn reference_item_type(preset: &d4lc_rules::AffixPreset) -> String {
    let mut item_types: Vec<String> = preset
        .affixes
        .iter()
        .map(|requirement| requirement.item_type.clone())
        .collect();
    item_types.sort();
    item_types.dedup();
    let rows_of = |item_type: &str| {
        preset
            .affixes
            .iter()
            .filter(|requirement| requirement.item_type == item_type)
            .collect::<Vec<_>>()
    };
    let mut best: Option<String> = None;
    for item_type in item_types {
        let rows = rows_of(&item_type);
        let mut ids: Vec<&str> = rows.iter().map(|row| row.affix_id.as_str()).collect();
        ids.sort();
        ids.dedup();
        if ids.len() != rows.len() {
            continue;
        }
        let better = match &best {
            None => true,
            Some(current) => rows.len() > rows_of(current).len(),
        };
        if better {
            best = Some(item_type);
        }
    }
    best.unwrap_or_else(|| {
        panic!(
            "an imported preset must carry at least one item type with \
             distinct affix ids, got {:?}",
            preset.affixes
        )
    })
}

/// The tooltip rows of a matching reference item: one row per requirement
/// of `item_type`, with the row kind its flags describe. The ids come from
/// the restarted preset, the types from the canonical item-type vocabulary
/// the parser produces, so the rows are what the evaluation sees at
/// runtime.
pub(super) fn reference_item_rows(
    preset: &d4lc_rules::AffixPreset,
    item_type: &str,
) -> Vec<d4lc_rules::ParsedAffixForRules> {
    preset
        .affixes
        .iter()
        .filter(|requirement| requirement.item_type == item_type)
        .map(|requirement| {
            let kind = if requirement.is_implicit {
                d4lc_rules::ParsedAffixKind::Implicit
            } else if requirement.is_tempered {
                d4lc_rules::ParsedAffixKind::Tempered
            } else if requirement.is_greater {
                d4lc_rules::ParsedAffixKind::Greater
            } else {
                d4lc_rules::ParsedAffixKind::Normal
            };
            d4lc_rules::ParsedAffixForRules {
                affix_id: requirement.affix_id.clone(),
                label: requirement.label.clone(),
                value_text: Some("+10.0%".to_string()),
                source_text: format!(
                    "+10.0% {}",
                    requirement
                        .label
                        .clone()
                        .unwrap_or_else(|| requirement.affix_id.clone())
                ),
                row_id: None,
                kind,
            }
        })
        .collect()
}

pub(super) const IMPORT_FILE_BODY: &str = r#"{
  "Name": "Speed",
  "ItemAffixes": [
    { "Id": "AttackSpeed", "Type": "Amulet", "Color": { "A": 255, "R": 34, "G": 197, "B": 94 }, "IsGreater": true, "IsImplicit": false, "IsTempered": false }
  ],
  "ItemAspects": [],
  "ItemSigils": [],
  "ItemUniques": [],
  "ItemRunes": []
}"#;
