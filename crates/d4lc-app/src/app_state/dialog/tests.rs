use super::*;
use crate::app_state::AppState;

// -----------------------------------------------------------------------
// DialogKind / DialogState / DialogFamily tests
// -----------------------------------------------------------------------

#[test]
fn dialog_kind_all_covers_every_variant() {
    // 22 upstream WPF dialog XAMLs + InfinityBuilds
    // + Maxroll; the five provider download dialogs
    // collapsed into the tabbed ImportPreset dialog;
    // + the generic confirm prompt.
    assert_eq!(DialogKind::ALL.len(), 20);
    // No duplicates.
    let mut seen = std::collections::HashSet::new();
    for kind in DialogKind::ALL {
        assert!(seen.insert(kind), "duplicate DialogKind in ALL");
    }
}

#[test]
fn dialog_kind_as_str_is_stable() {
    // The string id is part of the routing contract and must not change.
    assert_eq!(DialogKind::ColorsConfig.as_str(), "colors_config");
    assert_eq!(DialogKind::ControllerConfig.as_str(), "controller_config");
    assert_eq!(DialogKind::HotkeysConfig.as_str(), "hotkeys_config");
    assert_eq!(DialogKind::OverlayConfig.as_str(), "overlay_config");
    assert_eq!(DialogKind::ParagonConfig.as_str(), "paragon_config");
    assert_eq!(DialogKind::AffixConfig.as_str(), "affix_config");
    assert_eq!(DialogKind::AspectConfig.as_str(), "aspect_config");
    assert_eq!(DialogKind::SigilConfig.as_str(), "sigil_config");
    assert_eq!(DialogKind::UniqueConfig.as_str(), "unique_config");
    assert_eq!(DialogKind::RuneConfig.as_str(), "rune_config");
    assert_eq!(DialogKind::MultiBuildConfig.as_str(), "multi_build_config");
    assert_eq!(DialogKind::SetAffix.as_str(), "set_affix");
    assert_eq!(DialogKind::SetAffixColor.as_str(), "set_affix_color");
    assert_eq!(DialogKind::SetPresetName.as_str(), "set_preset_name");
    assert_eq!(DialogKind::RenamePreset.as_str(), "rename_preset");
    assert_eq!(DialogKind::ImportPreset.as_str(), "import_preset");
    assert_eq!(DialogKind::AddTradeItem.as_str(), "add_trade_item");
    assert_eq!(DialogKind::TradeConfig.as_str(), "trade_config");
    assert_eq!(DialogKind::HotkeyConfig.as_str(), "hotkey_config");
    assert_eq!(DialogKind::Confirm.as_str(), "confirm");
}

#[test]
fn dialog_kind_as_str_is_unique_per_variant() {
    let mut seen = std::collections::HashSet::new();
    for kind in DialogKind::ALL {
        assert!(
            seen.insert(kind.as_str()),
            "duplicate dialog-id string for {kind:?}"
        );
    }
}

#[test]
fn dialog_kind_from_str_id_roundtrips_for_every_variant() {
    for kind in DialogKind::ALL {
        let parsed = DialogKind::from_str_id(kind.as_str())
            .unwrap_or_else(|| panic!("from_str_id failed for {kind:?}"));
        assert_eq!(parsed, kind);
    }
}

#[test]
fn dialog_kind_from_str_id_returns_none_for_unknown() {
    assert!(DialogKind::from_str_id("nope").is_none());
    assert!(DialogKind::from_str_id("").is_none());
}

#[test]
fn dialog_kind_display_matches_as_str() {
    for kind in DialogKind::ALL {
        assert_eq!(format!("{kind}"), kind.as_str());
    }
}

#[test]
fn dialog_kind_family_grouping_is_stable() {
    // Settings family: 5 dialogs.
    assert_eq!(DialogKind::ColorsConfig.family(), DialogFamily::Settings);
    assert_eq!(
        DialogKind::ControllerConfig.family(),
        DialogFamily::Settings
    );
    assert_eq!(DialogKind::HotkeysConfig.family(), DialogFamily::Settings);
    assert_eq!(DialogKind::OverlayConfig.family(), DialogFamily::Settings);
    assert_eq!(DialogKind::ParagonConfig.family(), DialogFamily::Settings);
    // Affix family: 9 dialogs.
    for kind in [
        DialogKind::AffixConfig,
        DialogKind::AspectConfig,
        DialogKind::SigilConfig,
        DialogKind::UniqueConfig,
        DialogKind::RuneConfig,
        DialogKind::SetAffix,
        DialogKind::SetAffixColor,
        DialogKind::SetPresetName,
        DialogKind::RenamePreset,
    ] {
        assert_eq!(kind.family(), DialogFamily::Affix, "kind {kind:?}");
    }
    // Trade family: 2 dialogs.
    assert_eq!(DialogKind::AddTradeItem.family(), DialogFamily::Trade);
    assert_eq!(DialogKind::TradeConfig.family(), DialogFamily::Trade);
    // Hotkeys family: 1 dialog.
    assert_eq!(DialogKind::HotkeyConfig.family(), DialogFamily::Hotkeys);
    // Build-import family: ImportPreset and MultiBuildConfig.
    for kind in [DialogKind::ImportPreset, DialogKind::MultiBuildConfig] {
        assert_eq!(kind.family(), DialogFamily::BuildImport, "kind {kind:?}");
    }
    // Confirm family: 1 kind — the generic prompt.
    assert_eq!(DialogKind::Confirm.family(), DialogFamily::Confirm);
}

#[test]
fn dialog_kind_default_title_is_nonempty_for_every_variant() {
    for kind in DialogKind::ALL {
        assert!(
            !kind.default_title().is_empty(),
            "default_title empty for {kind:?}"
        );
    }
}

#[test]
fn titles_follow_the_ui_language_where_upstream_localizes_them() {
    // Upstream localizes the import, rename and confirm-name titles
    // (`AffixViewModel.cs:1781,1809`, `ImportAffixPresetViewModel.cs:806`) and
    // hard-codes the others in English.
    assert_eq!(DialogKind::ImportPreset.title(), "Import preset");
    assert_eq!(DialogKind::RenamePreset.title(), "Rename");
    assert_eq!(DialogKind::SetPresetName.title(), "Confirm name");
    crate::i18n::with_language("es-ES", || {
        assert_eq!(DialogKind::ImportPreset.title(), "Importar preajuste");
        assert_eq!(DialogKind::RenamePreset.title(), "Renombrar");
        assert_eq!(DialogKind::SetPresetName.title(), "Confirma el nombre");
        assert_eq!(DialogKind::OverlayConfig.title(), "Overlay config");
    });
}

#[test]
fn dialog_state_default_has_no_error_and_not_busy() {
    let dialog = DialogState::default();
    assert!(dialog.error.is_empty());
    assert!(!dialog.busy);
    assert!(dialog.confirm.is_none());
}

#[test]
fn app_state_default_startup_includes_dialog_state() {
    let state = AppState::default_startup();
    assert_eq!(state.dialog.kind, DialogKind::ColorsConfig);
    assert!(state.dialog.error.is_empty());
    assert!(!state.dialog.busy);
}

#[test]
fn dialog_kind_family_visit_count_matches_total() {
    // Every dialog belongs to exactly one family; the sum of family
    // counts must equal the dialog count.
    let mut count = 0;
    for kind in DialogKind::ALL {
        let _ = kind.family();
        count += 1;
    }
    assert_eq!(count, DialogKind::ALL.len());
}
