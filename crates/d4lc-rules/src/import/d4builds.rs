use crate::import::{
    affix_tuning_prisms, degree_rotation_text, ensure_cache_filename_safe, flatten_paragon_routes,
    is_raw_capture_file, load_affix_tuning_prisms, push_import_warning,
    replace_ascii_case_insensitive, sort_and_dedup_affix_requirements, sorted_json_files,
    ImportBuildDescriptor, ImportColors, ImportLookupTables, ImportUnmappedEntry,
    ImportVariantDescriptor, ImportWarning, ImportWorkflowResult, ImportedItemAffixes,
    ImportedParagonBoard, ImportedParagonRoute, EQUIPMENT_ITEM_TYPES,
};
use crate::model::{
    AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement, RuleNonAffixKind,
};
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const D4BUILDS_PROVIDER: &str = "D4Builds";

const D4BUILDS_LOAD_AVAILABLE_METHOD: &str = "BuildsManagerD4Builds.LoadAvailableD4BuildsBuilds";

const D4BUILDS_CREATE_PRESET_METHOD: &str = "BuildsManagerD4Builds.CreatePresetFromD4BuildsBuild";

const D4BUILDS_CONVERT_VARIANTS_METHOD: &str = "BuildsManagerD4Builds.ConvertBuildVariants";

const D4BUILDS_CONVERT_AFFIX_METHOD: &str = "BuildsManagerD4Builds.ConvertItemAffix";

const D4BUILDS_CONVERT_ASPECT_METHOD: &str = "BuildsManagerD4Builds.ConvertItemAspect";

const D4BUILDS_CONVERT_RUNE_METHOD: &str = "BuildsManagerD4Builds.ConvertItemRune";

const D4BUILDS_CONVERT_UNIQUE_METHOD: &str = "BuildsManagerD4Builds.ConvertItemUnique";

const D4BUILDS_PARAGON_METHOD: &str = "BuildsManagerD4Builds.GetAllParagonBoards";

const D4BUILDS_LANGUAGE: &str = "enUS";

pub fn load_available_d4builds_variants(
    builds_dir: &Path,
) -> Result<Vec<ImportVariantDescriptor>, String> {
    let mut variants = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let build = parse_d4builds_file(&path)?;
        for variant in build.variants {
            variants.push(ImportVariantDescriptor {
                source_build_id: build.id.clone(),
                source_build_name: build.name.clone(),
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

/// Mirrors upstream `LoadAvailableD4BuildsBuilds`: the stored builds
/// enumerated as rows (name, scraped date, variant names). Raw browser
/// captures (`{id}.raw.json`) are not build entities and stay out of the
/// list — the D4Builds flow stores converted entities only; the exclusion
/// guards legacy raw captures and the Mobalytics capture that share the
/// `.raw.json` suffix convention.
pub fn load_available_d4builds_builds(
    builds_dir: &Path,
) -> Result<Vec<ImportBuildDescriptor>, String> {
    let mut builds = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        if is_raw_capture_file(&path) {
            continue;
        }
        let build = parse_d4builds_file(&path)?;
        builds.push(ImportBuildDescriptor {
            variant_names: build.variants.iter().map(|v| v.name.clone()).collect(),
            id: build.id,
            name: build.name,
            url: String::new(),
            date: build.date,
            source_path: path.display().to_string(),
        });
    }
    builds.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(builds)
}

pub fn import_d4builds_build_variant(
    repo: &AssetRepository,
    source_path: &Path,
    variant_name: Option<&str>,
    settings: &AppSettings,
) -> Result<ImportWorkflowResult, String> {
    let colors = ImportColors::from(settings);
    let build = parse_d4builds_file(source_path)?;
    let source_build_id = build.id.clone();
    let source_build_name = build.name.clone();
    let variant = select_d4builds_variant(&build, variant_name)?;
    let lookups = ImportLookupTables::load(repo, D4BUILDS_LANGUAGE)?;
    let tuning_prisms_by_id = load_affix_tuning_prisms(repo, D4BUILDS_LANGUAGE)?;
    let preset_name = if variant.name.trim().is_empty() {
        build.name.clone()
    } else {
        variant.name.clone()
    };

    let mut items = Vec::new();
    let mut affix_preset = AffixPreset {
        name: preset_name.clone(),
        affixes: Vec::new(),
    };
    let mut warnings = Vec::new();
    let mut unmapped_entries = Vec::new();

    for (slot_name, item_type, affixes) in variant.item_slots() {
        let mut item_affixes = Vec::new();
        for (index, affix) in affixes.iter().enumerate() {
            match lookups.affix.match_candidate(&affix.affix_text) {
                Some(candidate) => {
                    let tuning_prisms = affix_tuning_prisms(&tuning_prisms_by_id, &candidate.id);
                    let requirement = AffixRequirement {
                        affix_id: candidate.id,
                        label: Some(candidate.label),
                        item_type: item_type.to_string(),
                        // Upstream `BuildsManagerD4Builds.cs:548-550`:
                        // tempered wins, then implicit, else normal (a greater
                        // flag only rides along, it picks no color).
                        color: if affix.is_tempered {
                            colors.tempered.clone()
                        } else if affix.is_implicit {
                            colors.implicit.clone()
                        } else {
                            colors.normal.clone()
                        },
                        is_any_type: false,
                        is_greater: affix.is_greater,
                        is_implicit: affix.is_implicit,
                        is_tempered: affix.is_tempered,
                        tuning_prisms,
                    };
                    item_affixes.push(requirement.clone());
                    affix_preset.affixes.push(requirement);
                }
                None => push_import_warning(
                    D4BUILDS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("{slot_name}[{index}].AffixText"),
                    affix.affix_text.clone(),
                    "affix text did not map to a canonical imported id".to_string(),
                    D4BUILDS_CONVERT_AFFIX_METHOD,
                ),
            }
        }
        if !item_affixes.is_empty() {
            items.push(ImportedItemAffixes {
                item_type: item_type.to_string(),
                affixes: item_affixes,
            });
        }
    }

    sort_and_dedup_affix_requirements(&mut affix_preset.affixes);

    let mut aspects = Vec::new();
    for (index, raw_aspect) in variant.aspect.iter().enumerate() {
        let cleaned = raw_aspect.replace("Aspect", "");
        match lookups.aspect.match_candidate(&cleaned) {
            Some(candidate) => {
                for item_type in EQUIPMENT_ITEM_TYPES {
                    aspects.push(NonAffixRequirement {
                        id: candidate.id.clone(),
                        item_type: Some((*item_type).to_string()),
                        color: colors.aspects.clone(),
                        kind: RuleNonAffixKind::Aspect,
                    });
                }
            }
            None => push_import_warning(
                D4BUILDS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Aspect[{index}]"),
                raw_aspect.clone(),
                "aspect name did not map to a canonical imported id".to_string(),
                D4BUILDS_CONVERT_ASPECT_METHOD,
            ),
        }
    }

    let mut runes = Vec::new();
    for (index, raw_rune) in variant.runes.iter().enumerate() {
        match lookups.rune.match_candidate(raw_rune) {
            Some(candidate) => runes.push(NonAffixRequirement {
                id: candidate.id,
                item_type: None,
                color: colors.runes.clone(),
                kind: RuleNonAffixKind::Rune,
            }),
            None => push_import_warning(
                D4BUILDS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Runes[{index}]"),
                raw_rune.clone(),
                "rune name did not map to a canonical imported id".to_string(),
                D4BUILDS_CONVERT_RUNE_METHOD,
            ),
        }
    }

    let mut uniques = Vec::new();
    for (index, raw_unique) in variant.uniques.iter().enumerate() {
        match lookups.unique.match_candidate(raw_unique) {
            Some(candidate) => uniques.push(NonAffixRequirement {
                id: candidate.id,
                item_type: None,
                color: colors.uniques.clone(),
                kind: RuleNonAffixKind::Unique,
            }),
            None => push_import_warning(
                D4BUILDS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Uniques[{index}]"),
                raw_unique.clone(),
                "unique name did not map to a canonical imported id".to_string(),
                D4BUILDS_CONVERT_UNIQUE_METHOD,
            ),
        }
    }

    let paragon_routes = if variant.paragon_boards.is_empty() {
        Vec::new()
    } else {
        vec![ImportedParagonRoute {
            step_name: None,
            boards: variant
                .paragon_boards
                .iter()
                .map(|board| ImportedParagonBoard {
                    index: 0,
                    name: board.name.clone(),
                    glyph: board.glyph.clone(),
                    rotation: board.rotation.clone(),
                    active_nodes: board
                        .nodes
                        .iter()
                        .enumerate()
                        .filter_map(|(index, active)| active.then_some(index))
                        .collect(),
                })
                .collect::<Vec<_>>(),
        }]
    };
    let paragon_boards = flatten_paragon_routes(&paragon_routes);

    let mut source_trace = vec![
        D4BUILDS_LOAD_AVAILABLE_METHOD.to_string(),
        D4BUILDS_CONVERT_VARIANTS_METHOD.to_string(),
        D4BUILDS_CREATE_PRESET_METHOD.to_string(),
    ];
    if !paragon_boards.is_empty() {
        source_trace.push(D4BUILDS_PARAGON_METHOD.to_string());
    }

    Ok(ImportWorkflowResult {
        provider: D4BUILDS_PROVIDER.to_string(),
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

/// The stored D4Builds build entity — the exact upstream
/// `D4Companion.Entities.D4BuildsBuild` shape, persisted as indented
/// PascalCase JSON at `{builds_dir}/{id}.json` (upstream `WriteIndented`).
/// Unlike upstream the port does NOT embed the converted `AffixPreset`:
/// the fuzzy site-text → internal-id mapping is re-derived at import time
/// by `import_d4builds_build_variant`, so the entity carries the cleaned
/// site texts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct D4BuildsBuildFile {
    pub date: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variants: Vec<D4BuildsVariantFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct D4BuildsVariantFile {
    pub name: String,
    #[serde(default)]
    pub helm: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub chest: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub gloves: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub pants: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub boots: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub amulet: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub ring: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub weapon: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub ranged: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub offhand: Vec<D4BuildsAffixFile>,
    #[serde(default)]
    pub aspect: Vec<String>,
    #[serde(default)]
    pub runes: Vec<String>,
    #[serde(default)]
    pub uniques: Vec<String>,
    #[serde(default)]
    pub paragon_boards: Vec<D4BuildsParagonBoardFile>,
}

impl D4BuildsVariantFile {
    fn item_slots(&self) -> [(&'static str, &'static str, &[D4BuildsAffixFile]); 10] {
        [
            ("Helm", "helm", &self.helm),
            ("Chest", "chest", &self.chest),
            ("Gloves", "gloves", &self.gloves),
            ("Pants", "pants", &self.pants),
            ("Boots", "boots", &self.boots),
            ("Amulet", "amulet", &self.amulet),
            ("Ring", "ring", &self.ring),
            ("Weapon", "weapon", &self.weapon),
            ("Ranged", "ranged", &self.ranged),
            ("Offhand", "offhand", &self.offhand),
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct D4BuildsAffixFile {
    pub affix_text: String,
    #[serde(default)]
    pub is_greater: bool,
    #[serde(default)]
    pub is_implicit: bool,
    #[serde(default)]
    pub is_tempered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct D4BuildsParagonBoardFile {
    pub name: String,
    pub glyph: String,
    pub rotation: String,
    #[serde(default)]
    pub nodes: Vec<bool>,
}

// ---------------------------------------------------------------------------
// D4Builds site-text conversion (upstream `ExportBuildVariants` +
// `ConvertBuildVariants`).
//
// The d4lc-app browser flow scrapes one d4builds.gg build page into a
// [`D4BuildsSiteBuild`] — the raw site values upstream's Selenium export
// reads (stat groups, gear names, gem slots, paragon tile class/style
// attributes). The conversion here is the pure remainder of the upstream
// download: the `GetAllAffixes` affix-text cleanup plus the
// `ConvertBuildVariants` fuzzy matching against the localized Data
// dictionaries, producing the stored [`D4BuildsBuildFile`] entity that
// `import_d4builds_build_variant` consumes unchanged. Upstream folds the
// matched internal ids into each variant's `AffixPreset` at download time;
// the port re-derives that preset at import time, so the download-time
// matching here surfaces unknown site texts as structured warnings and
// never blocks the stored entity.
// ---------------------------------------------------------------------------

/// Raw DOM extraction of one d4builds.gg build page — one serializable
/// snapshot per scrape, so recorded fixtures drive the offline conversion
/// tests without a browser.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct D4BuildsSiteBuild {
    pub build_id: String,
    /// `renameBuild` value (legacy page) or `builder__header__description`
    /// innerHTML.
    pub name: String,
    /// `builder__last__updated` innerText.
    pub date: String,
    pub variants: Vec<D4BuildsSiteVariant>,
}

/// Raw DOM extraction of one build variant (one `dropdown__option`
/// selection).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct D4BuildsSiteVariant {
    pub name: String,
    /// `.builder__stats__group.{itemType}` extractions, one per upstream
    /// `GetAllAffixes` slot key that yielded stats.
    pub affix_groups: Vec<D4BuildsSiteAffixGroup>,
    /// `builder__gear__name` innerHTML values containing "Aspect".
    pub aspects: Vec<String>,
    /// `builder__gear__name` innerHTML values NOT containing "Aspect".
    pub uniques: Vec<String>,
    /// `builder__gem__slot` innerHTML values of length 3 or 4.
    pub runes: Vec<String>,
    /// `paragon__board` extractions; empty unless the paragon tab was
    /// scraped (upstream `IsImportParagonD4BuildsEnabled`).
    pub paragon_boards: Vec<D4BuildsSiteParagonBoard>,
}

/// Raw stats of one `.builder__stats__group.{itemType}` element.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct D4BuildsSiteAffixGroup {
    /// The upstream `GetAllAffixes(itemType)` slot key, e.g. `Helm`,
    /// `ChestArmor`, `Dual-WieldWeapon1`.
    pub slot: String,
    pub affixes: Vec<D4BuildsSiteAffix>,
}

/// Raw values of one `builder__stat` element: the `.filled` innerText plus
/// the outerHTML flag probes of upstream `GetAllAffixes`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct D4BuildsSiteAffix {
    pub text: String,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
}

/// Raw values of one `paragon__board` element: the board name innerText,
/// the glyph innerText, the raw `style` attribute, and every
/// `paragon__board__tile` `class` attribute.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct D4BuildsSiteParagonBoard {
    pub name: String,
    pub glyph: String,
    pub style: String,
    pub tiles: Vec<String>,
}

/// One converted variant plus the structured warnings its unknown site
/// texts produced (upstream converts without warnings — FuzzierSharp
/// `ExtractOne` always matches; the port's thresholded lookup can miss and
/// mirrors the import-time warning wording).
#[derive(Debug, Clone, PartialEq)]
pub struct D4BuildsConvertedVariant {
    pub variant: D4BuildsVariantFile,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

/// The full conversion result: the storeable [`D4BuildsBuildFile`] entity
/// plus the collected warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct D4BuildsConvertedBuild {
    pub build: D4BuildsBuildFile,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

/// The localized fuzzy lookup indexes the D4Builds site-text conversion
/// matches against (upstream `InitAffixData` / `InitAspectData` /
/// `InitRuneData` / `InitUniqueData`). Reuses the import-time index
/// construction (`ImportLookupTables`) so download-time and import-time
/// matching agree.
pub struct D4BuildsConversionLookups {
    inner: ImportLookupTables,
}

impl D4BuildsConversionLookups {
    /// Load the enUS lookup indexes (upstream D4Builds conversion always
    /// reads the bundled `Data/*.enUS.json` dictionaries).
    pub fn load(repo: &AssetRepository) -> Result<Self, String> {
        Ok(Self {
            inner: ImportLookupTables::load(repo, D4BUILDS_LANGUAGE)?,
        })
    }

    /// Convert one scraped variant (upstream `ConvertBuildVariants` body
    /// for a single variant). Unknown site texts become structured
    /// warnings; a malformed paragon tile/style is an `Err` like upstream's
    /// unguarded `int.Parse`, which aborts the whole download.
    pub fn convert_variant(
        &self,
        site: &D4BuildsSiteVariant,
    ) -> Result<D4BuildsConvertedVariant, String> {
        let mut warnings = Vec::new();
        let mut unmapped_entries = Vec::new();
        let mut variant = D4BuildsVariantFile {
            name: site.name.clone(),
            ..D4BuildsVariantFile::default()
        };

        // Affixes per slot (upstream flattens the ten item types into
        // (itemType, affix) tuples before matching; the port converts
        // per group and appends Ring1/Ring2 and the five weapon groups
        // like upstream's AddRange order).
        for group in &site.affix_groups {
            let (target, slot_label) = match group.slot.as_str() {
                "Helm" => (&mut variant.helm, "Helm"),
                "ChestArmor" => (&mut variant.chest, "Chest"),
                "Gloves" => (&mut variant.gloves, "Gloves"),
                "Pants" => (&mut variant.pants, "Pants"),
                "Boots" => (&mut variant.boots, "Boots"),
                "Amulet" => (&mut variant.amulet, "Amulet"),
                "Ring1" | "Ring2" => (&mut variant.ring, "Ring"),
                "Weapon" | "BludgeoningWeapon" | "SlashingWeapon" | "Dual-WieldWeapon1"
                | "Dual-WieldWeapon2" => (&mut variant.weapon, "Weapon"),
                "RangedWeapon" => (&mut variant.ranged, "Ranged"),
                "Offhand" => (&mut variant.offhand, "Offhand"),
                // The scraper only queries the upstream slot keys.
                _ => continue,
            };
            for (index, affix) in group.affixes.iter().enumerate() {
                let cleaned =
                    clean_d4builds_affix_text(&affix.text, affix.is_implicit, affix.is_tempered);
                match self.inner.affix.match_candidate(&cleaned) {
                    Some(_) => target.push(D4BuildsAffixFile {
                        affix_text: cleaned,
                        is_greater: affix.is_greater,
                        is_implicit: affix.is_implicit,
                        is_tempered: affix.is_tempered,
                    }),
                    None => push_import_warning(
                        D4BUILDS_PROVIDER,
                        &mut warnings,
                        &mut unmapped_entries,
                        format!("{slot_label}[{index}].AffixText"),
                        affix.text.clone(),
                        "affix text did not map to a canonical imported id".to_string(),
                        D4BUILDS_CONVERT_AFFIX_METHOD,
                    ),
                }
            }
        }

        // Aspects (upstream strips the "Aspect" word OrdinalIgnoreCase
        // before matching).
        for (index, raw_aspect) in site.aspects.iter().enumerate() {
            let cleaned = replace_ascii_case_insensitive(raw_aspect, "Aspect", "");
            if self.inner.aspect.match_candidate(&cleaned).is_none() {
                push_import_warning(
                    D4BUILDS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Aspect[{index}]"),
                    raw_aspect.clone(),
                    "aspect name did not map to a canonical imported id".to_string(),
                    D4BUILDS_CONVERT_ASPECT_METHOD,
                );
            }
        }

        // Runes.
        for (index, raw_rune) in site.runes.iter().enumerate() {
            if self.inner.rune.match_candidate(raw_rune).is_none() {
                push_import_warning(
                    D4BUILDS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Runes[{index}]"),
                    raw_rune.clone(),
                    "rune name did not map to a canonical imported id".to_string(),
                    D4BUILDS_CONVERT_RUNE_METHOD,
                );
            }
        }

        // Uniques (upstream ignores matches under the 90 score — the empty
        // item slots — which the thresholded lookup already refuses).
        for (index, raw_unique) in site.uniques.iter().enumerate() {
            if self.inner.unique.match_candidate(raw_unique).is_none() {
                push_import_warning(
                    D4BUILDS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Uniques[{index}]"),
                    raw_unique.clone(),
                    "unique name did not map to a canonical imported id".to_string(),
                    D4BUILDS_CONVERT_UNIQUE_METHOD,
                );
            }
        }

        // Paragon boards (upstream converts the scraped tiles inside
        // `GetAllParagonBoards`; a malformed tile throws and fails the
        // download).
        if !site.paragon_boards.is_empty() {
            variant.paragon_boards = parse_d4builds_site_paragon_boards(&site.paragon_boards)?;
        }

        Ok(D4BuildsConvertedVariant {
            variant,
            warnings,
            unmapped_entries,
        })
    }
}

/// Convert a full scraped build into the stored entity (upstream
/// `ConvertBuildVariants` over every variant). Unknown site texts surface
/// as structured warnings; a malformed paragon board is an `Err` (upstream
/// exception parity).
pub fn convert_d4builds_site_variants(
    lookups: &D4BuildsConversionLookups,
    site: &D4BuildsSiteBuild,
) -> Result<D4BuildsConvertedBuild, String> {
    let mut build = D4BuildsBuildFile {
        date: site.date.clone(),
        id: site.build_id.clone(),
        name: site.name.clone(),
        variants: Vec::with_capacity(site.variants.len()),
    };
    let mut warnings = Vec::new();
    let mut unmapped_entries = Vec::new();
    for variant in &site.variants {
        let converted = lookups.convert_variant(variant)?;
        warnings.extend(converted.warnings);
        unmapped_entries.extend(converted.unmapped_entries);
        build.variants.push(converted.variant);
    }
    Ok(D4BuildsConvertedBuild {
        build,
        warnings,
        unmapped_entries,
    })
}

/// Parse the scraped paragon boards into the entity boards (upstream
/// `GetAllParagonBoards`): board name line filtering, glyph paren strip,
/// `rotate(...deg)` parsing, and the active-tile `rN`/`cN` transforms on
/// the 21x21 grid. Fails like upstream's unguarded `int.Parse` — a
/// malformed class/style attribute aborts the download.
pub fn parse_d4builds_site_paragon_boards(
    boards: &[D4BuildsSiteParagonBoard],
) -> Result<Vec<D4BuildsParagonBoardFile>, String> {
    let mut result = Vec::with_capacity(boards.len());
    for (board_index, board) in boards.iter().enumerate() {
        // Board name: first non-short line, else "unknown".
        let name = board
            .name
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .map(str::trim)
            .find(|line| line.chars().count() > 1)
            .unwrap_or("unknown")
            .to_string();
        let glyph = board.glyph.replace(['(', ')'], "");
        let rotate_degrees = parse_d4builds_rotate_degrees(&board.style, board_index)?;

        let mut nodes = vec![false; 21 * 21];
        for tile in &board.tiles {
            // Upstream probes the raw class string: `htmlclass.Contains("active")`.
            if !tile.contains("active") {
                continue;
            }
            let tokens: Vec<&str> = tile.split_whitespace().collect();
            if tokens.len() < 3 {
                return Err(format!(
                    "paragon tile class '{tile}' carries no r/c location (upstream \
                     GetAllParagonBoards index access)"
                ));
            }
            let location_y = digits_only(tokens[1])
                .ok_or_else(|| format!("paragon tile class '{tile}' has no row digits"))?
                as i64;
            let location_x = digits_only(tokens[2])
                .ok_or_else(|| format!("paragon tile class '{tile}' has no column digits"))?
                as i64;
            // The rotate transform chain of upstream `GetAllParagonBoards`;
            // a rotation outside the four known values falls through with
            // unchanged coordinates exactly like upstream's if/else chain.
            let (location_xt, location_yt) = match rotate_degrees {
                0 => (location_x - 1, location_y - 1),
                90 => (21 - location_y, location_x - 1),
                180 => (21 - location_x, 21 - location_y),
                270 => (location_y - 1, 21 - location_x),
                _ => (location_x, location_y),
            };
            if !(0..21).contains(&location_xt) || !(0..21).contains(&location_yt) {
                return Err(format!(
                    "paragon tile class '{tile}' lands outside the 21x21 grid \
                     (x{location_xt}/y{location_yt})"
                ));
            }
            nodes[(location_yt * 21 + location_xt) as usize] = true;
        }

        result.push(D4BuildsParagonBoardFile {
            name,
            glyph,
            rotation: degree_rotation_text(rotate_degrees).to_string(),
            nodes,
        });
    }
    Ok(result)
}

/// The `rotate(...deg)` value of a board `style` attribute, modulo 360
/// (upstream `GetAllParagonBoards` rotate-string conversion). A style
/// without `rotate(` means 0.
fn parse_d4builds_rotate_degrees(style: &str, board_index: usize) -> Result<i32, String> {
    let Some(begin) = style.find("rotate(") else {
        return Ok(0);
    };
    let value_start = begin + "rotate(".len();
    let Some(end_offset) = style[value_start..].find("deg)") else {
        return Err(format!(
            "paragon board {board_index} style '{style}' carries 'rotate(' without 'deg)'"
        ));
    };
    let raw = style[value_start..value_start + end_offset].trim();
    raw.parse::<i32>()
        .map(|degrees| degrees % 360)
        .map_err(|error| {
            format!("paragon board {board_index} rotate value '{raw}' is not an integer: {error}")
        })
}

/// The digits of one class token (`r12` → 12); `None` when the token
/// carries none (upstream `int.Parse("")` throws).
fn digits_only(token: &str) -> Option<usize> {
    let digits: String = token.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The upstream `GetAllAffixes` clean-string step: implicit texts keep the
/// part after the first `:`, tempered texts drop `[...]` and `(...)` groups,
/// and every digit / bracket / sign / separator character is removed.
fn clean_d4builds_affix_text(text: &str, is_implicit: bool, is_tempered: bool) -> String {
    let mut cleaned = text.to_string();
    if is_implicit {
        if let Some(colon) = cleaned.find(':') {
            cleaned = cleaned[colon + 1..].to_string();
        }
    } else if is_tempered {
        cleaned = drop_bracketed_groups(&cleaned, '[');
        cleaned = drop_bracketed_groups(&cleaned, '(');
    }
    cleaned
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '0'..='9' | '[' | ']' | '(' | ')' | '+' | '-' | '.' | ',' | '%'
            )
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Remove every non-empty `[...]` / `(...)` group including its brackets
/// (upstream `Regex.Replace(text, @"\[(.+?)\]", "")` and the `(...)`
/// sibling; non-greedy, non-nested).
fn drop_bracketed_groups(text: &str, open: char) -> String {
    let close = match open {
        '[' => ']',
        _ => ')',
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == open {
            let mut group = String::new();
            for inner in chars.by_ref() {
                if inner == close {
                    break;
                }
                group.push(inner);
            }
            if group.is_empty() {
                // `\[(.+?)\]` does not match an empty group: keep it.
                out.push(open);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Persist the converted entity as indented PascalCase JSON at
/// `{builds_dir}/{id}.json` (upstream `WriteIndented` save in
/// `DownloadD4BuildsBuild`).
pub fn export_d4builds_build_cache(
    build: &D4BuildsBuildFile,
    builds_dir: &Path,
) -> Result<PathBuf, String> {
    ensure_cache_filename_safe(&build.id)?;
    fs::create_dir_all(builds_dir)
        .map_err(|error| format!("failed to create {}: {error}", builds_dir.display()))?;
    let path = builds_dir.join(format!("{}.json", build.id));
    let text = serde_json::to_string_pretty(build)
        .map_err(|error| format!("failed to serialize D4Builds build: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(path)
}

fn parse_d4builds_file(path: &Path) -> Result<D4BuildsBuildFile, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let build: D4BuildsBuildFile = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let _ = &build.date;
    Ok(build)
}

fn select_d4builds_variant<'a>(
    build: &'a D4BuildsBuildFile,
    variant_name: Option<&str>,
) -> Result<&'a D4BuildsVariantFile, String> {
    if let Some(variant_name) = variant_name {
        return build
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "missing D4Builds variant '{variant_name}' in build '{}'",
                    build.id
                )
            });
    }
    build
        .variants
        .first()
        .ok_or_else(|| format!("build '{}' has no variants", build.id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
