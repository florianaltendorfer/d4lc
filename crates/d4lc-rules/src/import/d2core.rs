use crate::import::maxroll::maxroll_rotate_paragon_location;
use crate::import::{
    affix_tuning_prisms, flatten_paragon_routes, load_affix_tuning_prisms,
    load_exact_id_lookup_index, paragon_rotation_text, push_import_warning,
    replace_ascii_case_insensitive, sort_and_dedup_affix_requirements, sorted_json_files,
    unix_ms_to_utc_timestamp, ImportBuildDescriptor, ImportColors, ImportExactIdLookupIndex,
    ImportLocalizedData, ImportVariantDescriptor, ImportWorkflowResult, ImportedItemAffixes,
    ImportedParagonBoard, ImportedParagonRoute, EQUIPMENT_ITEM_TYPES,
    UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR,
};
use crate::model::{
    AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement, RuleNonAffixKind,
};
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::Deserialize;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const D2CORE_PROVIDER: &str = "D2Core";

const D2CORE_LOAD_AVAILABLE_METHOD: &str = "BuildsManagerD2Core.LoadAvailableD2CoreBuilds";

const D2CORE_CREATE_PRESET_METHOD: &str = "BuildsManagerD2Core.CreatePresetFromD2CoreBuild";

const D2CORE_AFFIX_LOOKUP_METHOD: &str = "AffixManager.GetAffixInfoByIdName";

const D2CORE_PARAGON_BOARD_METHOD: &str = "AffixManager.GetParagonBoardLocalisation";

const D2CORE_PARAGON_GLYPH_METHOD: &str = "AffixManager.GetParagonGlyphLocalisationByNumber";

pub fn load_available_d2core_variants(
    builds_dir: &Path,
) -> Result<Vec<ImportVariantDescriptor>, String> {
    let mut variants = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let mut build = parse_d2core_file(&path)?;
        normalize_d2core_variant_names(&mut build);
        for variant in build.data.variants {
            variants.push(ImportVariantDescriptor {
                source_build_id: build.data.id.clone(),
                source_build_name: build.data.title.clone(),
                variant_name: Some(variant.name),
            });
        }
    }
    variants.sort_by(|left, right| {
        left.source_build_id
            .cmp(&right.source_build_id)
            .then(left.variant_name.cmp(&right.variant_name))
    });
    Ok(variants)
}

/// Mirrors upstream `BuildsManagerD2Core.LoadAvailableD2CoreBuilds`: the
/// stored planner JSONs enumerated as build rows (`Name`, `Date` from
/// `_updateTime`, variant names). The build list of the tabbed import
/// dialog reads this instead of the flat variant list.
pub fn load_available_d2core_builds(
    builds_dir: &Path,
) -> Result<Vec<ImportBuildDescriptor>, String> {
    let mut builds = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let mut build = parse_d2core_file(&path)?;
        normalize_d2core_variant_names(&mut build);
        builds.push(ImportBuildDescriptor {
            id: build.data.id.clone(),
            name: build.data.title.clone(),
            url: String::new(),
            date: unix_ms_to_utc_timestamp(build.data._update_time),
            variant_names: build.data.variants.iter().map(|v| v.name.clone()).collect(),
            source_path: path.display().to_string(),
        });
    }
    builds.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(builds)
}

pub fn import_d2core_build_variant(
    repo: &AssetRepository,
    source_path: &Path,
    variant_name: Option<&str>,
    settings: &AppSettings,
) -> Result<ImportWorkflowResult, String> {
    import_d2core_build_variant_with_paragon(
        repo,
        source_path,
        variant_name,
        &settings.selected_affix_language,
        settings.is_import_paragon_d2core_enabled,
        &ImportColors::from(settings),
    )
}

fn import_d2core_build_variant_with_paragon(
    repo: &AssetRepository,
    source_path: &Path,
    variant_name: Option<&str>,
    language: &str,
    import_paragon: bool,
    colors: &ImportColors,
) -> Result<ImportWorkflowResult, String> {
    let mut build = parse_d2core_file(source_path)?;
    normalize_d2core_variant_names(&mut build);
    let source_build_id = build.data.id.clone();
    let source_build_name = build.data.title.clone();
    let variant = select_d2core_variant(&build, variant_name)?;
    let localized = ImportLocalizedData::load(repo, language).map_err(|error| error.to_string())?;
    let affix_lookup = &localized.d2core_affixes;
    let unique_lookup = &localized.d2core_uniques;
    let paragon_lookup = &localized.d2core_paragon;
    let tuning_prisms_by_id = load_affix_tuning_prisms(repo, language)?;
    let preset_name = if variant.name.trim().is_empty() {
        build.data.title.clone()
    } else {
        variant.name.clone()
    };

    let mut item_affixes = BTreeMap::<String, Vec<AffixRequirement>>::new();
    let mut affix_preset = AffixPreset {
        name: preset_name.clone(),
        affixes: Vec::new(),
    };
    let mut warnings = Vec::new();
    let mut unmapped_entries = Vec::new();
    let mut aspects = Vec::new();
    let mut runes = Vec::new();
    let mut uniques = Vec::new();

    for (slot_key, gear) in &variant.gear {
        let Some(item_type) = d2core_item_type(&gear.item_type) else {
            push_import_warning(
                D2CORE_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("gear.{slot_key}.itemType"),
                gear.item_type.clone(),
                // BuildsManagerD2Core.cs:350.
                format!(
                    "Imported D2Core build contains unknown itemtype id: {}.",
                    gear.item_type
                ),
                D2CORE_CREATE_PRESET_METHOD,
            );
            continue;
        };

        for socket in &gear.sockets {
            if socket.r#type != "rune" {
                continue;
            }
            let rune_id = format!("Item_{}", socket.key);
            if !runes
                .iter()
                .any(|entry: &NonAffixRequirement| entry.id == rune_id)
            {
                runes.push(NonAffixRequirement {
                    id: rune_id,
                    item_type: None,
                    // Upstream assigns no color to D2Core runes
                    // (`BuildsManagerD2Core.cs:362-364`), so the
                    // `ItemAffix.Color` default stays.
                    color: UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR.to_string(),
                    kind: RuleNonAffixKind::Rune,
                });
            }
        }

        if gear.r#type == "uniqueItem" {
            if let Some(unique) = unique_lookup.match_candidate(&gear.key) {
                uniques.push(NonAffixRequirement {
                    id: unique.id,
                    item_type: None,
                    color: colors.uniques.clone(),
                    kind: RuleNonAffixKind::Unique,
                });
            }
        }

        for (index, affix) in gear.mods.iter().enumerate() {
            let Some(candidate) = affix_lookup.match_id_name(&affix.name) else {
                push_import_warning(
                    D2CORE_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("gear.{slot_key}.mods[{index}].name"),
                    affix.name.clone(),
                    // BuildsManagerD2Core.cs:398.
                    format!(
                        "Imported D2Core build contains unknown affix: {}.",
                        affix.name
                    ),
                    D2CORE_AFFIX_LOOKUP_METHOD,
                );
                continue;
            };

            let tuning_prisms = affix_tuning_prisms(&tuning_prisms_by_id, &candidate.id);
            let requirement = AffixRequirement {
                affix_id: candidate.id,
                label: Some(candidate.label),
                item_type: item_type.to_string(),
                // Upstream `BuildsManagerD2Core.cs:409`: greater affixes take
                // the configured greater color, everything else the normal one.
                color: if affix.greater {
                    colors.greater.clone()
                } else {
                    colors.normal.clone()
                },
                is_any_type: false,
                is_greater: affix.greater,
                is_implicit: false,
                is_tempered: affix
                    .name
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Tempered_")),
                tuning_prisms,
            };

            let slot_affixes = match item_affixes.entry(item_type.to_string()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(Vec::new()),
            };
            if !slot_affixes.iter().any(|existing| {
                existing.affix_id == requirement.affix_id
                    && existing.item_type == requirement.item_type
                    && existing.is_implicit == requirement.is_implicit
                    && existing.is_tempered == requirement.is_tempered
            }) {
                slot_affixes.push(requirement.clone());
            }
            if !affix_preset.affixes.iter().any(|existing| {
                existing.affix_id == requirement.affix_id
                    && existing.item_type == requirement.item_type
                    && existing.is_implicit == requirement.is_implicit
                    && existing.is_tempered == requirement.is_tempered
            }) {
                affix_preset.affixes.push(requirement);
            }
        }

        if gear.r#type == "legendary" {
            let aspect_id = replace_ascii_case_insensitive(&gear.key, "Affix_", "");
            if !aspects
                .iter()
                .any(|entry: &NonAffixRequirement| entry.id == aspect_id)
            {
                for equipment_item_type in EQUIPMENT_ITEM_TYPES {
                    aspects.push(NonAffixRequirement {
                        id: aspect_id.clone(),
                        item_type: Some((*equipment_item_type).to_string()),
                        // Upstream assigns no color to D2Core aspects
                        // (`BuildsManagerD2Core.cs:432-441`); the
                        // `ItemAffix.Color` default stays.
                        color: UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR.to_string(),
                        kind: RuleNonAffixKind::Aspect,
                    });
                }
            }
        }
    }

    sort_and_dedup_affix_requirements(&mut affix_preset.affixes);

    let mut items = item_affixes
        .into_iter()
        .map(|(item_type, mut affixes)| {
            sort_and_dedup_affix_requirements(&mut affixes);
            ImportedItemAffixes { item_type, affixes }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.item_type.cmp(&right.item_type));

    let paragon_routes = if import_paragon {
        let mut paragon_boards = variant
            .paragon
            .iter()
            .map(|(board_id, board)| {
                let glyph_id = board.glyph.values().next().cloned().unwrap_or_default();
                (
                    board.index,
                    ImportedParagonBoard {
                        index: board.index,
                        name: paragon_lookup.board_name(board_id),
                        glyph: paragon_lookup.glyph_name(&glyph_id),
                        rotation: paragon_rotation_text(board.rotate.into()).to_string(),
                        active_nodes: d2core_rotate_paragon_nodes(&board.data, board.rotate),
                    },
                )
            })
            .collect::<Vec<_>>();
        paragon_boards.sort_by_key(|entry| entry.0);
        let boards = paragon_boards
            .into_iter()
            .map(|(_, board)| board)
            .collect::<Vec<_>>();
        if boards.is_empty() {
            Vec::new()
        } else {
            vec![ImportedParagonRoute {
                step_name: None,
                boards,
            }]
        }
    } else {
        Vec::new()
    };
    let paragon_boards = flatten_paragon_routes(&paragon_routes);

    let mut source_trace = vec![
        D2CORE_LOAD_AVAILABLE_METHOD.to_string(),
        D2CORE_CREATE_PRESET_METHOD.to_string(),
    ];
    if !paragon_boards.is_empty() {
        source_trace.push(D2CORE_PARAGON_BOARD_METHOD.to_string());
        source_trace.push(D2CORE_PARAGON_GLYPH_METHOD.to_string());
    }

    Ok(ImportWorkflowResult {
        provider: D2CORE_PROVIDER.to_string(),
        source_url_or_file: source_path.display().to_string(),
        source_build_id,
        source_build_name,
        class_name: None,
        variant_name: Some(variant.name.clone()),
        affix_preset_name: preset_name.clone(),
        items,
        aspects: aspects.clone(),
        uniques: uniques.clone(),
        runes: runes.clone(),
        affix_preset,
        non_affix_preset: NonAffixPreset {
            name: preset_name,
            powers: aspects
                .iter()
                .chain(uniques.iter())
                .cloned()
                .collect::<Vec<_>>(),
            runes,
            sigils: Vec::new(),
        },
        paragon_boards,
        paragon_routes,
        warnings,
        unmapped_entries,
        source_trace,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct D2CoreBuildRootFile {
    data: D2CoreBuildFile,
}

#[derive(Debug, Clone, Deserialize)]
struct D2CoreBuildFile {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    #[serde(default, rename = "_createTime")]
    _create_time: i64,
    #[serde(default)]
    variants: Vec<D2CoreVariantFile>,
    #[serde(default, rename = "_updateTime")]
    _update_time: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct D2CoreVariantFile {
    #[serde(default)]
    gear: BTreeMap<String, D2CoreGearFile>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    paragon: BTreeMap<String, D2CoreParagonBoardFile>,
}

/// Upstream deserializes the D2Core planner file through a global
/// `StringConverter` JSON converter that coerces every scalar to its
/// string form — the live planner emits numeric values for fields the
/// port models as strings (e.g. `"key": 0` on gear entries). Mirror
/// that tolerance for those fields.
fn deserialize_string_from_any<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringFromAny;
    impl serde::de::Visitor<'_> for StringFromAny {
        type Value = String;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or scalar the upstream StringConverter coerces to string")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            Ok(value.to_string())
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<String, E> {
            Ok(value.to_string())
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<String, E> {
            Ok(value.to_string())
        }
        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<String, E> {
            Ok(value.to_string())
        }
        fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<String, E> {
            Ok(value.to_string())
        }
    }
    deserializer.deserialize_any(StringFromAny)
}

#[derive(Debug, Clone, Default, Deserialize)]
struct D2CoreGearFile {
    #[serde(default, rename = "itemType")]
    item_type: String,
    #[serde(default, deserialize_with = "deserialize_string_from_any")]
    key: String,
    #[serde(default)]
    mods: Vec<D2CoreAffixFile>,
    #[serde(default)]
    sockets: Vec<D2CoreSocketFile>,
    #[serde(default, rename = "type")]
    r#type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct D2CoreAffixFile {
    #[serde(default)]
    greater: bool,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct D2CoreSocketFile {
    #[serde(default, deserialize_with = "deserialize_string_from_any")]
    key: String,
    #[serde(default, rename = "type")]
    r#type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct D2CoreParagonBoardFile {
    #[serde(default)]
    data: Vec<String>,
    #[serde(default)]
    glyph: BTreeMap<String, String>,
    #[serde(default)]
    index: i32,
    #[serde(default)]
    rotate: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct D2CoreParagonLookup {
    board: ImportExactIdLookupIndex,
    glyph: ImportExactIdLookupIndex,
}

impl D2CoreParagonLookup {
    pub(crate) fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Ok(Self {
            board: load_exact_id_lookup_index(repo, language, "ParagonBoards")?,
            glyph: load_exact_id_lookup_index(repo, language, "ParagonGlyphs")?,
        })
    }

    fn board_name(&self, id: &str) -> String {
        self.board
            .match_id_case_insensitive(id)
            .unwrap_or_else(|| id.to_string())
    }

    fn glyph_name(&self, id: &str) -> String {
        let Some(number) = id.split('_').next_back() else {
            return id.to_string();
        };
        self.glyph
            .match_id_contains(number)
            .unwrap_or_else(|| id.to_string())
    }
}

fn parse_d2core_file(path: &Path) -> Result<D2CoreBuildRootFile, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn normalize_d2core_variant_names(build: &mut D2CoreBuildRootFile) {
    for variant in &mut build.data.variants {
        if variant.name.trim().is_empty() {
            variant.name = build.data.title.clone();
        }
    }
}

fn select_d2core_variant<'a>(
    build: &'a D2CoreBuildRootFile,
    variant_name: Option<&str>,
) -> Result<&'a D2CoreVariantFile, String> {
    if let Some(variant_name) = variant_name {
        return build
            .data
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "missing D2Core variant '{variant_name}' in build '{}'",
                    build.data.id
                )
            });
    }
    build
        .data
        .variants
        .first()
        .ok_or_else(|| format!("build '{}' has no variants", build.data.id))
}

fn d2core_item_type(item_type: &str) -> Option<&'static str> {
    match item_type {
        "Helm" => Some("helm"),
        "ChestArmor" => Some("chest"),
        "DruidOffhand" | "Focus" | "Shield" => Some("offhand"),
        "Dagger" | "Glaive" | "Mace" | "Mace2H" | "Polearm" | "Quarterstaff" | "Scythe"
        | "Scythe2H" | "Staff" | "Sword" | "Sword2H" | "Wand" => Some("weapon"),
        "Bow" | "Crossbow2H" => Some("ranged"),
        "Gloves" => Some("gloves"),
        "Legs" => Some("pants"),
        "Boots" => Some("boots"),
        "Ring" => Some("ring"),
        "Amulet" => Some("amulet"),
        _ => None,
    }
}

fn d2core_rotate_paragon_nodes(nodes: &[String], rotation: i32) -> Vec<usize> {
    let mut active_nodes = Vec::new();
    for location_string in nodes {
        let mut parts = location_string.split('_');
        let Some(location_y) = parts.next().and_then(|part| part.parse::<usize>().ok()) else {
            continue;
        };
        let Some(location_x) = parts.next().and_then(|part| part.parse::<usize>().ok()) else {
            continue;
        };
        let location = location_x + location_y * 21;
        active_nodes.push(maxroll_rotate_paragon_location(location, rotation));
    }
    active_nodes.sort_unstable();
    active_nodes
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
