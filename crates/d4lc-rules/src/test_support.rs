use crate::import::ImportedParagonBoard;
use crate::model::{AffixRequirement, ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules};
use crate::paragon::ParagonOverlayLayout;
use d4lc_assets::catalog::IdentityCatalog;
use d4lc_core::AffixRowId;
use std::path::Path;

pub(crate) fn prism_requirement(affix_id: &str, prisms: &[&str]) -> AffixRequirement {
    AffixRequirement {
        affix_id: affix_id.to_string(),
        label: Some("Attack Speed".to_string()),
        item_type: "amulet".to_string(),
        color: "Green".to_string(),
        is_any_type: false,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: prisms.iter().map(|prism| prism.to_string()).collect(),
    }
}

pub(crate) fn prism_test_item(affix_id: &str) -> ParsedItemForRules {
    ParsedItemForRules {
        item_type: "amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: vec![ParsedAffixForRules {
            affix_id: affix_id.to_string(),
            label: Some("Attack Speed".to_string()),
            value_text: Some("+32.0%".to_string()),
            source_text: "+32.0% Attack Speed".to_string(),
            row_id: Some(AffixRowId(0)),
            kind: ParsedAffixKind::Normal,
        }],
    }
}

pub(crate) fn affix_requirement(affix_id: &str, item_type: &str, color: &str) -> AffixRequirement {
    AffixRequirement {
        affix_id: affix_id.to_string(),
        label: None,
        item_type: item_type.to_string(),
        color: color.to_string(),
        is_any_type: false,
        is_greater: false,
        is_implicit: false,
        is_tempered: false,
        tuning_prisms: Vec::new(),
    }
}

pub(crate) fn parsed_affix(affix_id: &str, kind: ParsedAffixKind) -> ParsedAffixForRules {
    parsed_affix_with_value(affix_id, kind, None, None)
}

pub(crate) fn parsed_affix_with_value(
    affix_id: &str,
    kind: ParsedAffixKind,
    value_text: Option<&str>,
    row_id: Option<AffixRowId>,
) -> ParsedAffixForRules {
    ParsedAffixForRules {
        affix_id: affix_id.to_string(),
        label: None,
        value_text: value_text.map(ToOwned::to_owned),
        source_text: affix_id.to_string(),
        row_id,
        kind,
    }
}

pub(crate) fn rect(x: i32, y: i32, width: u32, height: u32) -> d4lc_core::Rect {
    d4lc_core::Rect {
        x,
        y,
        width,
        height,
    }
}

pub(crate) fn default_paragon_overlay_layout(
    window_geometry: d4lc_core::Rect,
) -> ParagonOverlayLayout {
    ParagonOverlayLayout {
        window_geometry,
        panel_left: 0,
        panel_top: 100,
        panel_height: 50,
        panel_gap: 2,
        panel_text_offset: 20,
        font_px: 20,
        tile_gap: 5,
        tile_count: 21,
        node_size_full: 20,
        node_size_collapsed: 10,
        node_border_size: 2,
        collapsed_left_offset: 0,
        collapsed_top_offset: 0,
    }
}

pub(crate) fn sample_paragon_board(
    name: &str,
    glyph: &str,
    rotation: &str,
    active_nodes: &[usize],
) -> ImportedParagonBoard {
    ImportedParagonBoard {
        index: 0,
        name: name.to_string(),
        glyph: glyph.to_string(),
        rotation: rotation.to_string(),
        active_nodes: active_nodes.to_vec(),
    }
}

pub(crate) fn write_overlay_debug_json(file_name: &str, value: &serde_json::Value) {
    let path = workspace_root()
        .join("debug-output/overlay")
        .join(file_name);
    std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

pub(crate) fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|crates| crates.parent()) {
        Some(root) => root.to_path_buf(),
        None => panic!("could not resolve workspace root"),
    }
}

pub(crate) fn d4builds_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/imports/d4builds")
}

pub(crate) fn d2core_source_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/d2core/source")
}

/// Canonical affix id of one pinned-catalog lookup key (`AttackSpeed`, a
/// SNO, or a full `IdName`). Every import path stores this identity, not
/// the shortened `IdNameList` entry.
pub(crate) fn canonical_affix_id(key: &str) -> String {
    let repo = d4lc_assets::AssetRepository::new(workspace_root());
    IdentityCatalog::affixes(&repo, "enUS")
        .unwrap_or_else(|error| panic!("{error}"))
        .resolve(key)
        .resolved()
        .unwrap_or_else(|| panic!("{key} must resolve to one canonical affix identity"))
        .id_name
        .clone()
}

pub(crate) fn maxroll_source_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/maxroll/source")
}

pub(crate) fn mobalytics_source_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/mobalytics/source")
}

// -----------------------------------------------------------------------
// InfinityBuilds provider. No live network: the share
// page is a committed fixture and the cache round-trip runs through a
// temp directory.
// -----------------------------------------------------------------------

pub(crate) fn infinity_builds_fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/imports/infinitybuilds")
}

pub(crate) fn infinity_builds_share_fixture_dir() -> std::path::PathBuf {
    infinity_builds_fixture_dir().join("share")
}
