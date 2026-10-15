use crate::import::{
    affix_tuning_prisms, degree_rotation_text, ensure_cache_filename_safe, flatten_paragon_routes,
    is_raw_capture_file, load_affix_tuning_prisms, normalize_import_text, push_import_warning,
    sort_and_dedup_affix_requirements, sorted_json_files, ImportBuildDescriptor, ImportColors,
    ImportLookupCandidate, ImportLookupIndex, ImportLookupTables, ImportUnmappedEntry,
    ImportVariantDescriptor, ImportWarning, ImportWorkflowResult, ImportedItemAffixes,
    ImportedParagonBoard, ImportedParagonRoute, EQUIPMENT_ITEM_TYPES,
};
use crate::model::{
    AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement, RuleNonAffixKind,
};
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const MOBALYTICS_PROVIDER: &str = "Mobalytics";

const MOBALYTICS_PARSE_BUILD_METHOD: &str = "BuildsManagerMobalytics.ParseJsonBuild";

const MOBALYTICS_PARSE_BUILD_BY_SLUG_METHOD: &str = "BuildsManagerMobalytics.ParseJsonBuildBySlug";

const MOBALYTICS_CREATE_PRESET_METHOD: &str =
    "BuildsManagerMobalytics.CreatePresetFromMobalyticsBuild";

const MOBALYTICS_CONVERT_VARIANTS_METHOD: &str = "BuildsManagerMobalytics.ConvertBuildVariants";

const MOBALYTICS_CONVERT_AFFIX_METHOD: &str = "BuildsManagerMobalytics.ConvertItemAffix";

const MOBALYTICS_CONVERT_ASPECT_METHOD: &str = "BuildsManagerMobalytics.ConvertItemAspect";

const MOBALYTICS_CONVERT_RUNE_METHOD: &str = "BuildsManagerMobalytics.ConvertItemRune";

const MOBALYTICS_CONVERT_UNIQUE_METHOD: &str = "BuildsManagerMobalytics.ConvertItemUnique";

const MOBALYTICS_PARAGON_METHOD: &str = "BuildsManagerMobalytics.GetAllParagonBoards";

pub const MOBALYTICS_SHOW_BROWSER_DEFAULT: bool = false;

const MOBALYTICS_LANGUAGE: &str = "enUS";

pub fn load_available_mobalytics_variants(
    builds_dir: &Path,
) -> Result<Vec<ImportVariantDescriptor>, String> {
    let mut variants = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if let Some(profile) = parse_mobalytics_profile_json(&text)? {
            variants.extend(
                profile
                    .builds
                    .into_iter()
                    .map(|build| ImportVariantDescriptor {
                        source_build_id: build.id,
                        source_build_name: build.name.clone(),
                        variant_name: Some(build.name),
                    }),
            );
            continue;
        }
        if let Some((build, _)) = parse_mobalytics_stored_build(&text)? {
            variants.extend(
                build
                    .variants
                    .into_iter()
                    .map(|variant| ImportVariantDescriptor {
                        source_build_id: build.id.clone(),
                        source_build_name: build.name.clone(),
                        variant_name: Some(variant.name),
                    }),
            );
        }
    }
    variants.sort_by(|left, right| {
        left.source_build_id
            .cmp(&right.source_build_id)
            .then(left.source_build_name.cmp(&right.source_build_name))
            .then(left.variant_name.cmp(&right.variant_name))
    });
    Ok(variants)
}

/// Mirrors upstream `LoadAvailableMobalyticsBuilds`: stored builds as rows
/// (name, document `updatedAt`, variant names, stored page URL). Profile
/// captures surface their build summaries as rows (no date — the profile
/// summary carries none). Raw browser captures (`.raw.json`) stay out of
/// the list: they are legacy diagnostic caches, not importable builds.
pub fn load_available_mobalytics_builds(
    builds_dir: &Path,
) -> Result<Vec<ImportBuildDescriptor>, String> {
    let mut builds = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        if is_raw_capture_file(&path) {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if let Some(profile) = parse_mobalytics_profile_json(&text)? {
            builds.extend(
                profile
                    .builds
                    .into_iter()
                    .map(|build| ImportBuildDescriptor {
                        id: build.id,
                        name: build.name,
                        url: String::new(),
                        date: String::new(),
                        variant_names: Vec::new(),
                        source_path: String::new(),
                    }),
            );
            continue;
        }
        if let Some((build, _)) = parse_mobalytics_stored_build(&text)? {
            builds.push(ImportBuildDescriptor {
                variant_names: build.variants.iter().map(|v| v.name.clone()).collect(),
                id: build.id,
                name: build.name,
                url: build.url,
                date: build.date,
                source_path: path.display().to_string(),
            });
        }
    }
    builds.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(builds)
}

pub fn import_mobalytics_build_variant(
    repo: &AssetRepository,
    source_path: &Path,
    variant_name: Option<&str>,
    settings: &AppSettings,
) -> Result<ImportWorkflowResult, String> {
    let colors = ImportColors::from(settings);
    let text = fs::read_to_string(source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let (build, parse_method) = parse_mobalytics_stored_build(&text)?.ok_or_else(|| {
        format!(
            "failed to parse {} as a Mobalytics build",
            source_path.display()
        )
    })?;
    let variant = select_mobalytics_variant(&build, variant_name)?;
    let lookups = ImportLookupTables::load(repo, MOBALYTICS_LANGUAGE)?;
    let tuning_prisms_by_id = load_affix_tuning_prisms(repo, MOBALYTICS_LANGUAGE)?;
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
            match mobalytics_match_affix_tail_joins(&lookups.affix, &affix.affix_text) {
                Some(candidate) => {
                    let tuning_prisms = affix_tuning_prisms(&tuning_prisms_by_id, &candidate.id);
                    let requirement = AffixRequirement {
                        affix_id: candidate.id,
                        label: Some(candidate.label),
                        item_type: item_type.to_string(),
                        // Upstream `BuildsManagerMobalytics.cs:680-683`:
                        // implicit, then greater, then tempered, else normal.
                        color: colors.by_implicit_greater_tempered(
                            affix.is_implicit,
                            affix.is_greater,
                            affix.is_tempered,
                        ),
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
                    MOBALYTICS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("{slot_name}[{index}].AffixText"),
                    affix.affix_text.clone(),
                    "affix text did not map to a canonical imported id".to_string(),
                    MOBALYTICS_CONVERT_AFFIX_METHOD,
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
        match lookups.aspect.match_candidate(raw_aspect) {
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
                MOBALYTICS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Aspect[{index}]"),
                raw_aspect.clone(),
                "aspect name did not map to a canonical imported id".to_string(),
                MOBALYTICS_CONVERT_ASPECT_METHOD,
            ),
        }
    }

    let mut runes = Vec::new();
    for (index, raw_rune) in variant.runes.iter().enumerate() {
        match lookups.rune.match_candidate(raw_rune) {
            Some(candidate) => {
                if !runes
                    .iter()
                    .any(|entry: &NonAffixRequirement| entry.id == candidate.id)
                {
                    runes.push(NonAffixRequirement {
                        id: candidate.id,
                        item_type: None,
                        color: colors.runes.clone(),
                        kind: RuleNonAffixKind::Rune,
                    });
                }
            }
            None => push_import_warning(
                MOBALYTICS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Runes[{index}]"),
                raw_rune.clone(),
                "rune name did not map to a canonical imported id".to_string(),
                MOBALYTICS_CONVERT_RUNE_METHOD,
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
                MOBALYTICS_PROVIDER,
                &mut warnings,
                &mut unmapped_entries,
                format!("Uniques[{index}]"),
                raw_unique.clone(),
                "unique name did not map to a canonical imported id".to_string(),
                MOBALYTICS_CONVERT_UNIQUE_METHOD,
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
                .map(mobalytics_imported_paragon_board)
                .collect::<Vec<_>>(),
        }]
    };
    let paragon_boards = flatten_paragon_routes(&paragon_routes);

    let mut source_trace = vec![
        parse_method.to_string(),
        MOBALYTICS_CONVERT_VARIANTS_METHOD.to_string(),
        MOBALYTICS_CREATE_PRESET_METHOD.to_string(),
    ];
    if !paragon_boards.is_empty() {
        source_trace.push(MOBALYTICS_PARAGON_METHOD.to_string());
    }

    Ok(ImportWorkflowResult {
        provider: MOBALYTICS_PROVIDER.to_string(),
        source_url_or_file: source_path.display().to_string(),
        source_build_id: build.id.clone(),
        source_build_name: build.name.clone(),
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

// ---------------------------------------------------------------------------
// Mobalytics stored entities + download-time conversion.
//
// Upstream `BuildsManagerMobalytics.ParseJsonBuild` saves the CONVERTED
// `MobalyticsBuild` entity (indented JSON at `.\Builds\Mobalytics\{Id}.json`)
// and `ParseJsonProfile` saves the `MobalyticsProfile` entity
// (`.\Profiles\Mobalytics\{display name}.json`). The port stores the same
// entity shapes minus the converted `AffixPreset` (the D4Builds precedent:
// the fuzzy site-text → internal-id mapping is re-derived at import time by
// `import_mobalytics_build_variant`), so the download flow produces the
// entity here and the import consumes it unchanged.
// ---------------------------------------------------------------------------

/// The stored Mobalytics build entity — the upstream
/// `D4Companion.Entities.MobalyticsBuild` shape (PascalCase JSON keys so a
/// cache file round-trips like upstream `WriteIndented` saves).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsBuildFile {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub variants: Vec<MobalyticsVariantFile>,
}

/// Stored Mobalytics build variant (upstream `MobalyticsBuildVariant`
/// without the converted `AffixPreset`; the paragon boards carry the
/// converted 21x21 `Nodes` grid like upstream's `ParagonBoard` entity).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsVariantFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub helm: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub chest: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub gloves: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub pants: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub boots: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub amulet: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub ring: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub weapon: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub ranged: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub offhand: Vec<MobalyticsAffixEntry>,
    #[serde(default)]
    pub aspect: Vec<String>,
    #[serde(default)]
    pub uniques: Vec<String>,
    #[serde(default)]
    pub runes: Vec<String>,
    #[serde(default)]
    pub paragon_boards: Vec<MobalyticsParagonBoardFile>,
}

/// One raw site affix of a stored variant (upstream `MobalyticsAffix`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsAffixEntry {
    #[serde(default)]
    pub affix_text: String,
    #[serde(default)]
    pub affix_text_list: Vec<String>,
    #[serde(default)]
    pub is_greater: bool,
    #[serde(default)]
    pub is_implicit: bool,
    #[serde(default)]
    pub is_tempered: bool,
}

/// Stored paragon board (upstream `ParagonBoard` entity: rotation text and
/// the 21x21 boolean node grid).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsParagonBoardFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub glyph: String,
    #[serde(default)]
    pub rotation: String,
    #[serde(default)]
    pub nodes: Vec<bool>,
}

/// The stored Mobalytics profile entity — the upstream
/// `D4Companion.Entities.MobalyticsProfile` shape, persisted as indented
/// JSON at `{profiles_dir}/{name}.json` (upstream `ParseJsonProfile`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsProfileFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub profile_name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub variants: Vec<MobalyticsProfileVariant>,
}

/// One published build of a stored profile (upstream
/// `MobalyticsProfileBuildVariant`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MobalyticsProfileVariant {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
}

/// One converted variant plus the structured warnings its unknown site
/// texts produced (upstream converts without warnings — FuzzierSharp
/// `ExtractOne` always matches; the port's thresholded lookup can miss and
/// mirrors the import-time warning wording).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MobalyticsConvertedVariant {
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

/// The full conversion result: the storeable [`MobalyticsBuildFile`] entity
/// plus the collected warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobalyticsConvertedBuild {
    pub build: MobalyticsBuildFile,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

/// The localized fuzzy lookup indexes the Mobalytics conversion matches
/// against (upstream `InitAffixData` / `InitAspectData` / `InitRuneData` /
/// `InitUniqueData`). Reuses the import-time index construction
/// (`ImportLookupTables`) so download-time and import-time matching agree.
pub struct MobalyticsConversionLookups {
    inner: ImportLookupTables,
}

impl MobalyticsConversionLookups {
    /// Load the enUS lookup indexes (upstream Mobalytics conversion always
    /// reads the bundled `Data/*.enUS.json` dictionaries).
    pub fn load(repo: &AssetRepository) -> Result<Self, String> {
        Ok(Self {
            inner: ImportLookupTables::load(repo, MOBALYTICS_LANGUAGE)?,
        })
    }

    /// Convert one exported variant (upstream `ConvertBuildVariants` body
    /// for a single variant). The stored entity carries the raw site texts
    /// — the conversion validates every text against the localized
    /// dictionaries and reports the unmapped ones as structured warnings;
    /// the ids themselves are re-derived at import time.
    pub fn convert_variant(
        &self,
        variant: &MobalyticsVariantFile,
    ) -> Result<MobalyticsConvertedVariant, String> {
        let mut warnings = Vec::new();
        let mut unmapped_entries = Vec::new();

        for (slot_label, affixes) in variant_slot_entries(variant) {
            for (index, affix) in affixes.iter().enumerate() {
                if mobalytics_match_affix_tail_joins(&self.inner.affix, &affix.affix_text).is_none()
                {
                    push_import_warning(
                        MOBALYTICS_PROVIDER,
                        &mut warnings,
                        &mut unmapped_entries,
                        format!("{slot_label}[{index}].AffixText"),
                        affix.affix_text.clone(),
                        "affix text did not map to a canonical imported id".to_string(),
                        MOBALYTICS_CONVERT_AFFIX_METHOD,
                    );
                }
            }
        }

        for (index, raw_aspect) in variant.aspect.iter().enumerate() {
            if self.inner.aspect.match_candidate(raw_aspect).is_none() {
                push_import_warning(
                    MOBALYTICS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Aspect[{index}]"),
                    raw_aspect.clone(),
                    "aspect name did not map to a canonical imported id".to_string(),
                    MOBALYTICS_CONVERT_ASPECT_METHOD,
                );
            }
        }

        for (index, raw_rune) in variant.runes.iter().enumerate() {
            if self.inner.rune.match_candidate(raw_rune).is_none() {
                push_import_warning(
                    MOBALYTICS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Runes[{index}]"),
                    raw_rune.clone(),
                    "rune name did not map to a canonical imported id".to_string(),
                    MOBALYTICS_CONVERT_RUNE_METHOD,
                );
            }
        }

        for (index, raw_unique) in variant.uniques.iter().enumerate() {
            if self.inner.unique.match_candidate(raw_unique).is_none() {
                push_import_warning(
                    MOBALYTICS_PROVIDER,
                    &mut warnings,
                    &mut unmapped_entries,
                    format!("Uniques[{index}]"),
                    raw_unique.clone(),
                    "unique name did not map to a canonical imported id".to_string(),
                    MOBALYTICS_CONVERT_UNIQUE_METHOD,
                );
            }
        }

        Ok(MobalyticsConvertedVariant {
            warnings,
            unmapped_entries,
        })
    }
}

/// The stored variant's item slots in the upstream conversion order
/// (`ConvertBuildVariants` flattens Helm → Offhand).
fn variant_slot_entries(
    variant: &MobalyticsVariantFile,
) -> [(&'static str, &Vec<MobalyticsAffixEntry>); 10] {
    [
        ("Helm", &variant.helm),
        ("Chest", &variant.chest),
        ("Gloves", &variant.gloves),
        ("Pants", &variant.pants),
        ("Boots", &variant.boots),
        ("Amulet", &variant.amulet),
        ("Ring", &variant.ring),
        ("Weapon", &variant.weapon),
        ("Ranged", &variant.ranged),
        ("Offhand", &variant.offhand),
    ]
}

/// Export stage of the Mobalytics build download (upstream
/// `ParseJsonBuild` and `ExportBuildVariants`): parse the captured GraphQL
/// JSON into the stored entity shape. Accepts the `userGeneratedDocumentById`
/// root of the CDP `api/diablo4` responses AND the `apollo.graphqlV2`
/// wrapper of the slug-flow `__PRELOADED_STATE__`
/// (`UserGeneratedDocumentBySlug`, falling back to
/// `UserGeneratedDocumentBySlugifiedName`) — `Ok(None)` when neither shape
/// is present. The paragon boards are exported only when `paragon_enabled`
/// (upstream `ExportBuildVariant`,
/// `IsImportParagonMobalyticsEnabled`).
pub fn parse_mobalytics_graphql_build(
    graphql_json: &str,
    build_url: &str,
    paragon_enabled: bool,
) -> Result<Option<MobalyticsBuildFile>, String> {
    Ok(
        parse_mobalytics_graphql_build_with_warnings(graphql_json, build_url, paragon_enabled)?
            .map(|(build, _warnings)| build),
    )
}

/// [`parse_mobalytics_graphql_build`] plus the messages upstream reports
/// while exporting the build: `No nodes found for paragon board {name}.` for
/// every exported board without a node slug (`ErrorOccurredMessage`,
/// `BuildsManagerMobalytics.cs:967-974`). Nothing is reported when the
/// paragon export is off.
pub fn parse_mobalytics_graphql_build_with_warnings(
    graphql_json: &str,
    build_url: &str,
    paragon_enabled: bool,
) -> Result<Option<(MobalyticsBuildFile, Vec<String>)>, String> {
    let Some((internal, _method)) = parse_mobalytics_build_json(graphql_json)? else {
        return Ok(None);
    };
    let mut build = mobalytics_build_file_from_internal(&internal, build_url);
    let mut warnings = Vec::new();
    if paragon_enabled {
        for variant in &internal.variants {
            for board in variant
                .paragon_boards
                .iter()
                .filter(|board| !board.has_nodes)
            {
                warnings.push(format!("No nodes found for paragon board {}.", board.name));
            }
        }
    } else {
        for variant in &mut build.variants {
            variant.paragon_boards = Vec::new();
        }
    }
    Ok(Some((build, warnings)))
}

/// Convert a captured Mobalytics GraphQL JSON body into the stored entity
/// plus the conversion warnings (upstream `ParseJsonBuild` +
/// `ExportBuildVariants` + `ConvertBuildVariants`, fused). Unknown site
/// texts surface as structured warnings.
pub fn convert_mobalytics_graphql_build(
    graphql_json: &str,
    build_url: &str,
    paragon_enabled: bool,
    lookups: &MobalyticsConversionLookups,
) -> Result<Option<MobalyticsConvertedBuild>, String> {
    let Some(build) = parse_mobalytics_graphql_build(graphql_json, build_url, paragon_enabled)?
    else {
        return Ok(None);
    };
    let mut warnings = Vec::new();
    let mut unmapped_entries = Vec::new();
    for variant in &build.variants {
        let converted = lookups.convert_variant(variant)?;
        warnings.extend(converted.warnings);
        unmapped_entries.extend(converted.unmapped_entries);
    }
    Ok(Some(MobalyticsConvertedBuild {
        build,
        warnings,
        unmapped_entries,
    }))
}

/// Map the internal parsed build to the stored entity shape.
fn mobalytics_build_file_from_internal(
    internal: &MobalyticsBuild,
    build_url: &str,
) -> MobalyticsBuildFile {
    MobalyticsBuildFile {
        date: internal.date.clone(),
        id: internal.id.clone(),
        name: internal.name.clone(),
        url: build_url.to_string(),
        variants: internal
            .variants
            .iter()
            .map(|variant| MobalyticsVariantFile {
                name: variant.name.clone(),
                helm: mobalytics_affix_entries(&variant.helm),
                chest: mobalytics_affix_entries(&variant.chest),
                gloves: mobalytics_affix_entries(&variant.gloves),
                pants: mobalytics_affix_entries(&variant.pants),
                boots: mobalytics_affix_entries(&variant.boots),
                amulet: mobalytics_affix_entries(&variant.amulet),
                ring: mobalytics_affix_entries(&variant.ring),
                weapon: mobalytics_affix_entries(&variant.weapon),
                ranged: mobalytics_affix_entries(&variant.ranged),
                offhand: mobalytics_affix_entries(&variant.offhand),
                aspect: variant.aspect.clone(),
                uniques: variant.uniques.clone(),
                runes: variant.runes.clone(),
                paragon_boards: variant
                    .paragon_boards
                    .iter()
                    .map(mobalytics_paragon_board_file)
                    .collect(),
            })
            .collect(),
    }
}

fn mobalytics_affix_entries(affixes: &[MobalyticsAffixFile]) -> Vec<MobalyticsAffixEntry> {
    affixes
        .iter()
        .map(|affix| MobalyticsAffixEntry {
            affix_text_list: affix.affix_text.split('-').map(str::to_string).collect(),
            affix_text: affix.affix_text.clone(),
            is_greater: affix.is_greater,
            is_implicit: affix.is_implicit,
            is_tempered: affix.is_tempered,
        })
        .collect()
}

fn mobalytics_paragon_board_file(
    board: &MobalyticsParagonBoardExport,
) -> MobalyticsParagonBoardFile {
    let mut nodes = vec![false; 21 * 21];
    for location in &board.active_nodes {
        if *location < nodes.len() {
            nodes[*location] = true;
        }
    }
    MobalyticsParagonBoardFile {
        name: board.name.clone(),
        glyph: board.glyph.clone(),
        rotation: degree_rotation_text(board.rotation).to_string(),
        nodes,
    }
}

/// Map a stored entity back to the internal parsed shape the import path
/// consumes. The rotation text round-trips through the digit prefix
/// ("270°" → 270; "?°" → -1, rendered back as "?°").
fn mobalytics_internal_from_file(file: MobalyticsBuildFile) -> MobalyticsBuild {
    MobalyticsBuild {
        id: file.id,
        name: file.name,
        date: file.date,
        url: file.url,
        variants: file
            .variants
            .into_iter()
            .map(|variant| MobalyticsBuildVariant {
                name: variant.name,
                helm: mobalytics_internal_affixes(variant.helm),
                chest: mobalytics_internal_affixes(variant.chest),
                gloves: mobalytics_internal_affixes(variant.gloves),
                pants: mobalytics_internal_affixes(variant.pants),
                boots: mobalytics_internal_affixes(variant.boots),
                amulet: mobalytics_internal_affixes(variant.amulet),
                ring: mobalytics_internal_affixes(variant.ring),
                weapon: mobalytics_internal_affixes(variant.weapon),
                ranged: mobalytics_internal_affixes(variant.ranged),
                offhand: mobalytics_internal_affixes(variant.offhand),
                aspect: variant.aspect,
                uniques: variant.uniques,
                runes: variant.runes,
                paragon_boards: variant
                    .paragon_boards
                    .into_iter()
                    .map(|board| MobalyticsParagonBoardExport {
                        has_nodes: true,
                        name: board.name,
                        glyph: board.glyph,
                        rotation: board
                            .rotation
                            .trim_end_matches('°')
                            .parse::<i32>()
                            .unwrap_or(-1),
                        active_nodes: board
                            .nodes
                            .iter()
                            .enumerate()
                            .filter(|(_, active)| **active)
                            .map(|(location, _)| location)
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn mobalytics_internal_affixes(affixes: Vec<MobalyticsAffixEntry>) -> Vec<MobalyticsAffixFile> {
    affixes
        .into_iter()
        .map(|affix| MobalyticsAffixFile {
            affix_text: affix.affix_text,
            is_greater: affix.is_greater,
            is_implicit: affix.is_implicit,
            is_tempered: affix.is_tempered,
        })
        .collect()
}

/// Parse a stored Mobalytics build cache file: the converted entity shape
/// first, then the legacy raw GraphQL shapes (the pre-conversion fixtures
/// and the interim caches). `Ok(None)` when neither matches.
fn parse_mobalytics_stored_build(
    text: &str,
) -> Result<Option<(MobalyticsBuild, &'static str)>, String> {
    if let Ok(file) = serde_json::from_str::<MobalyticsBuildFile>(text) {
        // An entity with variants cannot be confused with a profile
        // summary or the raw shapes (all-default parses yield no variants).
        if !file.variants.is_empty() {
            return Ok(Some((
                mobalytics_internal_from_file(file),
                MOBALYTICS_PARSE_BUILD_METHOD,
            )));
        }
    }
    parse_mobalytics_build_json(text)
}

/// Persist the stored entity as indented JSON at `{builds_dir}/{id}.json`
/// (upstream `ParseJsonBuild`: `WriteIndented` save, file named by the
/// document id).
pub fn export_mobalytics_build_cache(
    build: &MobalyticsBuildFile,
    builds_dir: &Path,
) -> Result<PathBuf, String> {
    ensure_cache_filename_safe(&build.id)?;
    fs::create_dir_all(builds_dir)
        .map_err(|error| format!("failed to create {}: {error}", builds_dir.display()))?;
    let path = builds_dir.join(format!("{}.json", build.id));
    let text = serde_json::to_string_pretty(build)
        .map_err(|error| format!("failed to serialize Mobalytics build: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(path)
}

/// Port-side profile file-name gate: upstream names profile files after the
/// display name, which may contain spaces and any other non-separator
/// character — only path separators and the dot entries are refused.
fn ensure_profile_filename_safe(name: &str) -> Result<(), String> {
    let safe = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
    if safe {
        Ok(())
    } else {
        Err(format!(
            "profile name '{name}' cannot be used as a cache file name"
        ))
    }
}

/// Persist the stored profile as indented JSON at
/// `{profiles_dir}/{name}.json` (upstream `ParseJsonProfile` names the file
/// after the display name incl. the query-filter suffix).
pub fn export_mobalytics_profile_cache(
    profile: &MobalyticsProfileFile,
    profiles_dir: &Path,
) -> Result<PathBuf, String> {
    ensure_profile_filename_safe(&profile.name)?;
    fs::create_dir_all(profiles_dir)
        .map_err(|error| format!("failed to create {}: {error}", profiles_dir.display()))?;
    let path = profiles_dir.join(format!("{}.json", profile.name));
    let text = serde_json::to_string_pretty(profile)
        .map_err(|error| format!("failed to serialize Mobalytics profile: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(path)
}

/// Mirrors upstream `LoadAvailableMobalyticsProfiles`: the stored profiles
/// of `profiles_dir` (`.\Profiles\Mobalytics`), plus one message per file
/// that could not be read or parsed. Upstream aborts the whole scan at the
/// first bad file and only logs it, leaving an enumeration-order-dependent
/// partial list; here every bad file is skipped and reported, so one
/// corrupt profile never hides the others. Blank files (skipped silently
/// upstream) and files without a usable profile name are skipped without
/// a message, so a stray JSON never becomes a phantom row. `Err` only when
/// the directory itself cannot be listed.
pub fn load_available_mobalytics_profiles(
    profiles_dir: &Path,
) -> Result<(Vec<MobalyticsProfileFile>, Vec<String>), String> {
    let mut profiles = Vec::new();
    let mut skipped = Vec::new();
    for path in sorted_json_files(profiles_dir)? {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                skipped.push(format!("failed to read {}: {error}", path.display()));
                continue;
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<MobalyticsProfileFile>(&text) {
            Ok(profile) if !profile.name.trim().is_empty() => profiles.push(profile),
            Ok(_) => {}
            Err(error) => skipped.push(format!("failed to parse {}: {error}", path.display())),
        }
    }
    Ok((profiles, skipped))
}

/// Remove a stored profile file by display NAME — upstream
/// `RemoveMobalyticsProfile` deletes `.\Profiles\Mobalytics\{name}.json`.
/// Returns whether a file was removed; a missing file is not an error (the
/// caller refreshes the list either way).
pub fn remove_mobalytics_profile(profiles_dir: &Path, profile_name: &str) -> Result<bool, String> {
    ensure_profile_filename_safe(profile_name)?;
    let path = profiles_dir.join(format!("{profile_name}.json"));
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Parse the `window.__PRELOADED_STATE__` `diablo4State` JSON of a profile
/// page into the stored profile entity (upstream `ParseJsonProfile`).
///
/// `Ok(None)` mirrors upstream's `Failed. Invalid json.` branch (the
/// `ngf-creator-profile-documents` / `mgp-header` queries are missing);
/// `Err` mirrors a deserialization exception (`Failed. See log.`).
///
/// Parity notes: the display name gains a ` - {value}` suffix per URL query
/// parameter; each variant's URL uses the `slugifiedName` when non-empty,
/// else the build id (upstream's `??` only fires on an explicit JSON null —
/// an empty slug would render an unusable URL, so the port falls back on
/// empty too); variants sort date descending.
pub fn parse_mobalytics_profile_state(
    diablo4_state_json: &str,
    url: &str,
) -> Result<Option<MobalyticsProfileFile>, String> {
    let state: Value = serde_json::from_str(diablo4_state_json)
        .map_err(|error| format!("Mobalytics profile state was not valid JSON: {error}"))?;
    let Some(queries) = state
        .get("apollo")
        .and_then(|apollo| apollo.get("graphqlV2"))
        .and_then(|graphql| graphql.get("queries"))
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };

    let query_builds = queries.iter().find(|query| {
        query
            .get("queryKey")
            .and_then(Value::as_array)
            .is_some_and(|keys| mobalytics_query_key_matches(keys, "ngf-creator-profile-documents"))
    });
    let query_profile = queries.iter().find(|query| {
        query
            .get("queryKey")
            .and_then(Value::as_array)
            .is_some_and(|keys| mobalytics_query_key_matches(keys, "mgp-header"))
    });
    let (Some(query_builds), Some(query_profile)) = (query_builds, query_profile) else {
        return Ok(None);
    };

    let user = query_profile
        .get("state")
        .and_then(|state| state.get("data"))
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .and_then(|entry| entry.get("mgp"))
        .and_then(|mgp| mgp.get("profile"))
        .and_then(|profile| profile.get("data"))
        .and_then(|data| data.get("user"));
    let profile_id = user
        .and_then(|user| user.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let display_name = user
        .and_then(|user| user.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let username = user
        .and_then(|user| user.get("username"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Parse url (upstream appends every `?a=b` query value to the name).
    let mut name = display_name;
    let filters = url.split('?').nth(1).unwrap_or_default();
    for filter in filters.split('&') {
        if !filter.contains('=') {
            continue;
        }
        let value = filter.split('=').nth(1).unwrap_or_default();
        name = format!("{name} - {value}");
    }

    let mut variants = Vec::new();
    let Some(pages) = query_builds
        .get("state")
        .and_then(|state| state.get("data"))
        .and_then(|data| data.get("pages"))
        .and_then(Value::as_array)
    else {
        return Ok(Some(MobalyticsProfileFile {
            id: profile_id,
            name,
            profile_name: username.clone(),
            url: url.to_string(),
            variants,
        }));
    };
    for page_group in pages {
        let Some(page_group) = page_group.as_array() else {
            continue;
        };
        for page in page_group {
            let Some(documents) = page
                .get("game")
                .and_then(|game| game.get("documents"))
                .and_then(|documents| documents.get("userGeneratedDocuments"))
                .and_then(|documents| documents.get("documents"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for build in documents {
                // Upstream skips null entries (`if build == null continue`).
                let Some(id) = build.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(build_name) = build
                    .get("data")
                    .and_then(|data| data.get("name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let slug = build
                    .get("slugifiedName")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                variants.push(MobalyticsProfileVariant {
                    date: build
                        .get("updatedAt")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    id: id.to_string(),
                    name: build_name.to_string(),
                    url: format!(
                        "https://mobalytics.gg/diablo-4/profile/{username}/builds/{}",
                        if slug.is_empty() { id } else { slug }
                    ),
                });
            }
        }
    }

    // Sort builds by date, newest first (upstream
    // `DateTime.Parse(y).CompareTo(DateTime.Parse(x))`; the site emits
    // uniform ISO-8601 Z stamps, whose lexical order is chronological).
    variants.sort_by(|left, right| right.date.cmp(&left.date));

    Ok(Some(MobalyticsProfileFile {
        id: profile_id,
        name,
        profile_name: username,
        url: url.to_string(),
        variants,
    }))
}

#[derive(Debug, Clone)]
struct MobalyticsBuild {
    id: String,
    name: String,
    /// The document `updatedAt` stamp (upstream `MobalyticsBuild.Date`).
    date: String,
    /// The page URL the build was downloaded from (upstream
    /// `MobalyticsBuild.Url`); empty for the legacy raw parses that carry
    /// no URL (the stored entity path injects the file's own `Url`).
    url: String,
    variants: Vec<MobalyticsBuildVariant>,
}

#[derive(Debug, Clone)]
struct MobalyticsBuildVariant {
    name: String,
    helm: Vec<MobalyticsAffixFile>,
    chest: Vec<MobalyticsAffixFile>,
    gloves: Vec<MobalyticsAffixFile>,
    pants: Vec<MobalyticsAffixFile>,
    boots: Vec<MobalyticsAffixFile>,
    amulet: Vec<MobalyticsAffixFile>,
    ring: Vec<MobalyticsAffixFile>,
    weapon: Vec<MobalyticsAffixFile>,
    ranged: Vec<MobalyticsAffixFile>,
    offhand: Vec<MobalyticsAffixFile>,
    aspect: Vec<String>,
    uniques: Vec<String>,
    runes: Vec<String>,
    paragon_boards: Vec<MobalyticsParagonBoardExport>,
}

impl MobalyticsBuildVariant {
    fn item_slots(&self) -> [(&str, &str, &[MobalyticsAffixFile]); 10] {
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

#[derive(Debug, Clone)]
struct MobalyticsAffixFile {
    affix_text: String,
    is_greater: bool,
    is_implicit: bool,
    is_tempered: bool,
}

#[derive(Debug, Clone)]
struct MobalyticsParagonBoardExport {
    name: String,
    glyph: String,
    rotation: i32,
    active_nodes: Vec<usize>,
    /// Whether the build carried any node slug for this board
    /// (`BuildsManagerMobalytics.cs:967-974`). Only the fresh GraphQL parse
    /// knows; a board read back from a stored file counts as having nodes.
    has_nodes: bool,
}

#[derive(Debug, Clone)]
struct MobalyticsProfileBuildSummary {
    id: String,
    name: String,
}

#[derive(Debug, Clone)]
struct MobalyticsProfileSummary {
    builds: Vec<MobalyticsProfileBuildSummary>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildRootJson {
    data: MobalyticsBuildDataJson,
}

/// Mobalytics' GraphQL sends `null` for empty lists and texts (e.g.
/// `implicitStats: null`, live 2026-10); `null` reads as the default.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A list whose `null` entries are skipped: Mobalytics marks an empty
/// gear-stat position with `null` (live 2026-10); a `null` list is empty.
fn null_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<Option<T>>>::deserialize(deserializer)?
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect())
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildDataJson {
    game: MobalyticsBuildGameJson,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildGameJson {
    documents: MobalyticsBuildDocumentsJson,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildDocumentsJson {
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "userGeneratedDocumentById"
    )]
    user_generated_document_by_id: MobalyticsBuildDocumentWrapperJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsBuildDocumentWrapperJson {
    #[serde(default, deserialize_with = "null_as_default")]
    data: MobalyticsBuildDocumentJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsBuildDocumentJson {
    id: String,
    data: MobalyticsBuildDocumentDataJson,
    #[serde(default, deserialize_with = "null_as_default", rename = "updatedAt")]
    _updated_at: String,
    #[serde(default, deserialize_with = "null_list")]
    content: Vec<MobalyticsBuildContentJson>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsBuildDocumentDataJson {
    name: String,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "buildVariants"
    )]
    build_variants: MobalyticsBuildVariantsJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsBuildVariantsJson {
    #[serde(default, deserialize_with = "null_list", rename = "values")]
    values: Vec<MobalyticsBuildVariantJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildVariantJson {
    id: String,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "genericBuilder"
    )]
    generic_builder: MobalyticsGenericBuilderJson,
    #[serde(default, deserialize_with = "null_as_default")]
    paragon: MobalyticsParagonJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsGenericBuilderJson {
    #[serde(default, deserialize_with = "null_list")]
    slots: Vec<MobalyticsSlotJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsSlotJson {
    #[serde(rename = "gameSlotSlug")]
    game_slot_slug: String,
    #[serde(rename = "gameEntity")]
    game_entity: MobalyticsGameEntityJson,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsGameEntityJson {
    #[serde(default, deserialize_with = "null_as_default")]
    slug: String,
    #[serde(default, deserialize_with = "null_as_default")]
    _title: String,
    #[serde(default, deserialize_with = "null_as_default")]
    r#type: String,
    #[serde(default, deserialize_with = "null_as_default")]
    modifiers: MobalyticsModifiersJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsModifiersJson {
    #[serde(default, deserialize_with = "null_list", rename = "gearStats")]
    gear_stats: Vec<MobalyticsGearStatJson>,
    #[serde(default, deserialize_with = "null_list", rename = "implicitStats")]
    implicit_stats: Vec<MobalyticsNamedModifierJson>,
    #[serde(default, deserialize_with = "null_list", rename = "socketStats")]
    socket_stats: Vec<MobalyticsSocketStatJson>,
    #[serde(default, deserialize_with = "null_list", rename = "temperingStats")]
    tempering_stats: Vec<MobalyticsNamedModifierJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsGearStatJson {
    id: String,
    #[serde(default, deserialize_with = "null_as_default", rename = "isGreater")]
    is_greater: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsNamedModifierJson {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsSocketStatJson {
    slug: String,
    #[serde(default, deserialize_with = "null_as_default")]
    r#type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsParagonJson {
    #[serde(default, deserialize_with = "null_list")]
    nodes: Vec<MobalyticsParagonNodeJson>,
    #[serde(default, deserialize_with = "null_list")]
    boards: Vec<MobalyticsParagonBoardJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsParagonNodeJson {
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsParagonBoardJson {
    rotation: i32,
    board: MobalyticsParagonBoardRefJson,
    glyph: Option<MobalyticsParagonGlyphRefJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsParagonBoardRefJson {
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsParagonGlyphRefJson {
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildContentJson {
    #[serde(default, deserialize_with = "null_as_default", rename = "__typename")]
    typename: String,
    #[serde(default, deserialize_with = "null_as_default")]
    data: MobalyticsBuildContentDataJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MobalyticsBuildContentDataJson {
    #[serde(default, deserialize_with = "null_list", rename = "childrenVariants")]
    children_variants: Vec<MobalyticsBuildContentVariantTitleJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsBuildContentVariantTitleJson {
    id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsProfileRootJson {
    apollo: MobalyticsProfileApolloJson,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsProfileApolloJson {
    #[serde(rename = "graphqlV2")]
    graphql_v2: MobalyticsProfileGraphqlJson,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsProfileGraphqlJson {
    #[serde(default)]
    queries: Vec<MobalyticsProfileQueryJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobalyticsProfileQueryJson {
    #[serde(default, rename = "queryKey")]
    query_key: Vec<Value>,
    state: Value,
}

fn parse_mobalytics_build_json(
    text: &str,
) -> Result<Option<(MobalyticsBuild, &'static str)>, String> {
    // The CDP id flow: the `userGeneratedDocumentById` GraphQL body. A body
    // of that shape that does not read is a real parse error, not "no
    // document".
    match serde_json::from_str::<MobalyticsBuildRootJson>(text) {
        Ok(root) => {
            return Ok(Some((
                build_mobalytics_build(
                    &root.data.game.documents.user_generated_document_by_id.data,
                ),
                MOBALYTICS_PARSE_BUILD_METHOD,
            )))
        }
        Err(error)
            if text
                .trim_start()
                .starts_with(r#"{"data":{"game":{"documents":{"userGeneratedDocumentById""#) =>
        {
            return Err(format!(
                "{MOBALYTICS_PARSE_BUILD_METHOD}: the Mobalytics build document could not be read: \
                 {error}"
            ));
        }
        Err(_) => {}
    }

    // The slug flow: the preloaded state's `apollo.graphqlV2` queries. The
    // state carries unrelated queries of other shapes (banners, comments
    // with an object `data`), so every query is inspected on its own and
    // only the by-slug documents are read.
    let Ok(wrapper) = serde_json::from_str::<Value>(text) else {
        return Ok(None);
    };
    let Some(queries) = wrapper
        .pointer("/apollo/graphqlV2/queries")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    for entry in queries
        .iter()
        .filter_map(|query| query.pointer("/state/data").and_then(Value::as_array))
        .flatten()
    {
        let Some(documents) = entry.pointer("/game/documents") else {
            continue;
        };
        for key in [
            "userGeneratedDocumentBySlug",
            "userGeneratedDocumentBySlugifiedName",
        ] {
            let Some(document) = documents.get(key) else {
                continue;
            };
            let document: MobalyticsBuildDocumentWrapperJson =
                serde_json::from_value(document.clone()).map_err(|error| {
                    format!(
                        "{MOBALYTICS_PARSE_BUILD_BY_SLUG_METHOD}: the Mobalytics {key} document \
                         could not be read: {error}"
                    )
                })?;
            if !document.data.id.trim().is_empty() {
                return Ok(Some((
                    build_mobalytics_build(&document.data),
                    MOBALYTICS_PARSE_BUILD_BY_SLUG_METHOD,
                )));
            }
        }
    }

    Ok(None)
}

/// Id of the builder state Mobalytics keeps next to the named variants.
const MOBALYTICS_PLACEHOLDER_VARIANT_ID: &str = "no-variant-id";

/// The unnamed `no-variant-id` builder state without any gear or paragon:
/// the site does not list it as a variant (live 2026-10, e.g.
/// `/diablo-4/builds/warlock-blazing-scream`), and upstream would export it
/// as an empty variant named after the build. Skipped; a placeholder that
/// carries gear or paragon data is kept like upstream.
fn is_empty_placeholder_variant(variant: &MobalyticsBuildVariantJson) -> bool {
    variant.id == MOBALYTICS_PLACEHOLDER_VARIANT_ID
        && variant.generic_builder.slots.is_empty()
        && variant.paragon.boards.is_empty()
        && variant.paragon.nodes.is_empty()
}

fn build_mobalytics_build(document: &MobalyticsBuildDocumentJson) -> MobalyticsBuild {
    MobalyticsBuild {
        id: document.id.clone(),
        name: document.data.name.clone(),
        date: document._updated_at.clone(),
        url: String::new(),
        variants: document
            .data
            .build_variants
            .values
            .iter()
            .filter(|variant| !is_empty_placeholder_variant(variant))
            .map(|variant| build_mobalytics_variant(document, variant))
            .collect(),
    }
}

fn build_mobalytics_variant(
    document: &MobalyticsBuildDocumentJson,
    variant: &MobalyticsBuildVariantJson,
) -> MobalyticsBuildVariant {
    let variant_name = document
        .content
        .iter()
        .find(|content| content.typename == "NgfDocumentCmWidgetContentVariantsV1")
        .and_then(|content| {
            content
                .data
                .children_variants
                .iter()
                .find(|child| child.id == variant.id)
        })
        .map(|child| child.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| document.data.name.clone());

    MobalyticsBuildVariant {
        name: variant_name,
        helm: mobalytics_collect_affixes(variant, "helm"),
        chest: mobalytics_collect_affixes(variant, "chest-armor"),
        gloves: mobalytics_collect_affixes(variant, "gloves"),
        pants: mobalytics_collect_affixes(variant, "pants"),
        boots: mobalytics_collect_affixes(variant, "boots"),
        amulet: mobalytics_collect_affixes(variant, "amulet"),
        ring: {
            let mut affixes = mobalytics_collect_affixes(variant, "ring-1");
            affixes.extend(mobalytics_collect_affixes(variant, "ring-2"));
            dedup_mobalytics_affixes(&mut affixes);
            affixes
        },
        weapon: {
            let mut affixes = Vec::new();
            for slot in [
                "bludgeoning-weapon",
                "dual-wield-weapon-1",
                "dual-wield-weapon-2",
                "slashing-weapon",
                "weapon",
            ] {
                affixes.extend(mobalytics_collect_affixes(variant, slot));
            }
            dedup_mobalytics_affixes(&mut affixes);
            affixes
        },
        ranged: mobalytics_collect_affixes(variant, "ranged-weapon"),
        offhand: mobalytics_collect_affixes(variant, "offhand"),
        aspect: mobalytics_collect_aspects(variant),
        uniques: mobalytics_collect_uniques(variant),
        runes: mobalytics_collect_runes(variant),
        paragon_boards: mobalytics_collect_paragon_boards(variant),
    }
}

fn mobalytics_collect_affixes(
    variant: &MobalyticsBuildVariantJson,
    item_type: &str,
) -> Vec<MobalyticsAffixFile> {
    let Some(slot) = variant
        .generic_builder
        .slots
        .iter()
        .find(|slot| slot.game_slot_slug.eq_ignore_ascii_case(item_type))
    else {
        return Vec::new();
    };

    let mut affixes = Vec::new();
    for stat in &slot.game_entity.modifiers.gear_stats {
        affixes.push(MobalyticsAffixFile {
            affix_text: stat.id.clone(),
            is_greater: stat.is_greater,
            is_implicit: false,
            is_tempered: false,
        });
    }
    for stat in &slot.game_entity.modifiers.implicit_stats {
        affixes.push(MobalyticsAffixFile {
            affix_text: stat.id.clone(),
            is_greater: false,
            is_implicit: true,
            is_tempered: false,
        });
    }
    for stat in &slot.game_entity.modifiers.tempering_stats {
        affixes.push(MobalyticsAffixFile {
            affix_text: stat.id.clone(),
            is_greater: false,
            is_implicit: false,
            is_tempered: true,
        });
    }
    affixes
}

fn dedup_mobalytics_affixes(affixes: &mut Vec<MobalyticsAffixFile>) {
    let mut deduped = Vec::new();
    for affix in affixes.drain(..) {
        if !deduped.iter().any(|existing: &MobalyticsAffixFile| {
            existing.affix_text == affix.affix_text
                && existing.is_greater == affix.is_greater
                && existing.is_implicit == affix.is_implicit
                && existing.is_tempered == affix.is_tempered
        }) {
            deduped.push(affix);
        }
    }
    *affixes = deduped;
}

fn mobalytics_collect_aspects(variant: &MobalyticsBuildVariantJson) -> Vec<String> {
    variant
        .generic_builder
        .slots
        .iter()
        .filter(|slot| slot.game_entity.r#type.eq_ignore_ascii_case("aspects"))
        .map(|slot| {
            slot.game_entity
                .slug
                .replace("aspect", "")
                .replace('-', " ")
        })
        .map(|value| normalize_import_text(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn mobalytics_collect_runes(variant: &MobalyticsBuildVariantJson) -> Vec<String> {
    let mut runes = Vec::new();
    for slot in &variant.generic_builder.slots {
        for socket in &slot.game_entity.modifiers.socket_stats {
            if !socket.r#type.eq_ignore_ascii_case("runes") {
                continue;
            }
            if !runes.iter().any(|entry| entry == &socket.slug) {
                runes.push(socket.slug.clone());
            }
        }
    }
    runes
}

fn mobalytics_collect_uniques(variant: &MobalyticsBuildVariantJson) -> Vec<String> {
    variant
        .generic_builder
        .slots
        .iter()
        .filter(|slot| slot.game_entity.r#type.eq_ignore_ascii_case("uniqueItems"))
        .map(|slot| normalize_import_text(&slot.game_entity.slug.replace('-', " ")))
        .filter(|value| !value.is_empty())
        .collect()
}

fn mobalytics_collect_paragon_boards(
    variant: &MobalyticsBuildVariantJson,
) -> Vec<MobalyticsParagonBoardExport> {
    let mut boards = Vec::new();
    for board in &variant.paragon.boards {
        let board_name = normalize_mobalytics_board_slug(&board.board.slug);
        let rotation = board.rotation.rem_euclid(360);
        let mut active_nodes = Vec::new();
        let has_nodes = variant
            .paragon
            .nodes
            .iter()
            .any(|node| node.slug.starts_with(&board_name));
        for node in variant
            .paragon
            .nodes
            .iter()
            .filter(|node| node.slug.starts_with(&board_name))
        {
            let Some(node_position) = node.slug.strip_prefix(&format!("{board_name}-")) else {
                continue;
            };
            let mut parts = node_position.split('-');
            let Some(location_x) = parts
                .next()
                .and_then(|part| part.strip_prefix('x'))
                .and_then(|part| part.parse::<usize>().ok())
            else {
                continue;
            };
            let Some(location_y) = parts
                .next()
                .and_then(|part| part.strip_prefix('y'))
                .and_then(|part| part.parse::<usize>().ok())
            else {
                continue;
            };
            let location = mobalytics_rotate_paragon_location(location_x, location_y, rotation);
            active_nodes.push(location);
        }
        active_nodes.sort_unstable();
        boards.push(MobalyticsParagonBoardExport {
            has_nodes,
            name: board_name,
            glyph: board
                .glyph
                .as_ref()
                .map(|glyph| glyph.slug.clone())
                .unwrap_or_default(),
            rotation,
            active_nodes,
        });
    }
    boards
}

fn mobalytics_rotate_paragon_location(
    location_x: usize,
    location_y: usize,
    rotation: i32,
) -> usize {
    let (mut location_xt, mut location_yt) = (location_x, location_y);
    match rotation {
        0 | 360 => {
            location_xt = location_xt.saturating_sub(1);
            location_yt = location_yt.saturating_sub(1);
        }
        90 => {
            location_xt = 21usize.saturating_sub(location_y);
            location_yt = location_x.saturating_sub(1);
        }
        180 => {
            location_xt = 21usize.saturating_sub(location_x);
            location_yt = 21usize.saturating_sub(location_y);
        }
        270 => {
            location_xt = location_y.saturating_sub(1);
            location_yt = 21usize.saturating_sub(location_x);
        }
        _ => {}
    }
    location_yt * 21 + location_xt
}

fn mobalytics_imported_paragon_board(board: &MobalyticsParagonBoardExport) -> ImportedParagonBoard {
    ImportedParagonBoard {
        index: 0,
        name: board.name.clone(),
        glyph: board.glyph.clone(),
        rotation: degree_rotation_text(board.rotation).to_string(),
        active_nodes: board.active_nodes.clone(),
    }
}

fn normalize_mobalytics_board_slug(slug: &str) -> String {
    slug.replace("barbarian-starter-board", "barbarian-starting-board")
        .replace("druid-starter-board", "druid-starting-board")
        .replace("necromancer-starter-board", "necromancer-starting-board")
        .replace("paladin-starter-board", "paladin-starting-board")
        .replace("rogue-starter-board", "rogue-starting-board")
        .replace("sorcerer-starter-board", "sorcerer-starting-board")
        .replace("spiritborn-starter-board", "spiritborn-starting-board")
        .replace("warlock-starter-board", "warlock-starting-board")
}

fn select_mobalytics_variant<'a>(
    build: &'a MobalyticsBuild,
    variant_name: Option<&str>,
) -> Result<&'a MobalyticsBuildVariant, String> {
    if let Some(variant_name) = variant_name {
        return build
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "missing Mobalytics variant '{variant_name}' in build '{}'",
                    build.id
                )
            });
    }
    build
        .variants
        .first()
        .ok_or_else(|| format!("build '{}' has no variants", build.id))
}

/// Mirrors upstream `BuildsManagerMobalytics.ConvertItemAffix`: the
/// Mobalytics affix `Id` is split on '-' (`MobalyticsAffix.AffixTextList`)
/// and every tail-join of the token list — last token only, then the last
/// two tokens, ... up to all tokens joined with ' ' — is fuzzy-matched.
/// The winner is the best fuzzy score, ties broken by the longer joined
/// affix text, remaining ties keep the earlier (shorter) tail, matching the
/// stable `OrderByDescending(score).ThenByDescending(affix.Length).First()`
/// of the upstream C# code. This replaces the deleted upstream
/// `CleanAffixText` prefix-scrub path; the Rust port never carried the
/// hardcoded prefix list, only text normalization, which
/// `ImportLookupIndex::match_candidate_with_score` applies internally.
fn mobalytics_match_affix_tail_joins(
    index: &ImportLookupIndex,
    affix_text: &str,
) -> Option<ImportLookupCandidate> {
    let tokens: Vec<&str> = affix_text.split('-').collect();
    let mut best: Option<(ImportLookupCandidate, f64, usize)> = None;
    for take in 1..=tokens.len() {
        let joined = tokens[tokens.len() - take..].join(" ");
        let Some((candidate, score)) = index.match_candidate_with_score(&joined) else {
            continue;
        };
        let joined_length = joined.chars().count();
        let is_better = match &best {
            None => true,
            Some((_, best_score, best_length)) => {
                score > *best_score || (score == *best_score && joined_length > *best_length)
            }
        };
        if is_better {
            best = Some((candidate, score, joined_length));
        }
    }
    best.map(|(candidate, _, _)| candidate)
}

fn parse_mobalytics_profile_json(text: &str) -> Result<Option<MobalyticsProfileSummary>, String> {
    let root = match serde_json::from_str::<MobalyticsProfileRootJson>(text) {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };

    let query_builds = root.apollo.graphql_v2.queries.iter().find(|query| {
        mobalytics_query_key_matches(&query.query_key, "ngf-creator-profile-documents")
    });
    let query_profile = root
        .apollo
        .graphql_v2
        .queries
        .iter()
        .find(|query| mobalytics_query_key_matches(&query.query_key, "mgp-header"));

    let (Some(query_builds), Some(_query_profile)) = (query_builds, query_profile) else {
        return Ok(None);
    };

    let pages = query_builds
        .state
        .get("data")
        .and_then(|value| value.get("pages"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut builds = Vec::new();
    for page_group in pages {
        let Some(page_group) = page_group.as_array() else {
            continue;
        };
        for page in page_group {
            let Some(documents) = page
                .get("game")
                .and_then(|value| value.get("documents"))
                .and_then(|value| value.get("userGeneratedDocuments"))
                .and_then(|value| value.get("documents"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for build in documents {
                let Some(id) = build.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(name) = build
                    .get("data")
                    .and_then(|value| value.get("name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                builds.push(MobalyticsProfileBuildSummary {
                    id: id.to_string(),
                    name: name.to_string(),
                });
            }
        }
    }

    if builds.is_empty() {
        return Ok(None);
    }

    Ok(Some(MobalyticsProfileSummary { builds }))
}

fn mobalytics_query_key_matches(query_key: &[Value], expected: &str) -> bool {
    query_key
        .iter()
        .any(|value| value.as_str() == Some(expected))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
