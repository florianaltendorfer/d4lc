use crate::import::{
    affix_tuning_prisms, ensure_localized_data_file, flatten_paragon_routes,
    load_affix_tuning_prisms, load_exact_id_lookup_index, push_import_warning,
    sort_and_dedup_affix_requirements, sorted_json_files, string_field, typed_localized_data_error,
    ImportBuildDescriptor, ImportBuildList, ImportColors, ImportExactIdLookupIndex,
    ImportLocalizedData, ImportLocalizedDataError, ImportUnmappedEntry, ImportVariantDescriptor,
    ImportWarning, ImportWorkflowResult, ImportedItemAffixes, ImportedParagonBoard,
    ImportedParagonRoute, LocalizedDataComponent, EQUIPMENT_ITEM_TYPES,
    UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR,
};
use crate::model::{
    AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement, RuleNonAffixKind,
};
use d4lc_assets::catalog::IdentityCatalog;
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

const MAXROLL_PROVIDER: &str = "Maxroll";

const MAXROLL_LOAD_AVAILABLE_METHOD: &str = "BuildsManagerMaxroll.LoadAvailableMaxrollBuilds";

const MAXROLL_CREATE_PRESET_METHOD: &str = "BuildsManagerMaxroll.CreatePresetFromMaxrollBuild";

pub fn load_available_maxroll_variants(
    builds_dir: &Path,
) -> Result<Vec<ImportVariantDescriptor>, String> {
    let mut variants = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let build = parse_maxroll_build_file(&path)?;
        for profile in &build.data.profiles {
            variants.push(ImportVariantDescriptor {
                source_build_id: build.id.clone(),
                source_build_name: build.name.clone(),
                variant_name: Some(profile.name.clone()),
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

/// Mirrors upstream `LoadAvailableMaxrollBuilds`: the stored planner
/// envelopes enumerated as build rows (name, envelope `date`, profile
/// names as variants). One unparseable file is skipped with a structured
/// warning naming it instead of failing the whole list — upstream keeps
/// the list usable the same way by wrapping the enumeration in a
/// try/catch that logs the bad file.
pub fn load_available_maxroll_builds(builds_dir: &Path) -> Result<ImportBuildList, String> {
    let mut list = ImportBuildList::default();
    for path in sorted_json_files(builds_dir)? {
        let build = match parse_maxroll_build_file(&path) {
            Ok(build) => build,
            Err(error) => {
                list.warnings.push(ImportWarning {
                    provider: MAXROLL_PROVIDER.to_string(),
                    source_field: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    source_value: path.display().to_string(),
                    message: error,
                    upstream_method: MAXROLL_LOAD_AVAILABLE_METHOD.to_string(),
                });
                continue;
            }
        };
        list.builds.push(ImportBuildDescriptor {
            variant_names: build
                .data
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            id: build.id,
            name: build.name,
            url: String::new(),
            date: build.date,
            source_path: path.display().to_string(),
        });
    }
    list.builds.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(list)
}

pub fn import_maxroll_build_profile(
    repo: &AssetRepository,
    source_path: &Path,
    profile_name: Option<&str>,
    settings: &AppSettings,
) -> Result<ImportWorkflowResult, String> {
    let build = parse_maxroll_build_file(source_path)?;
    let source_build_id = build.id.clone();
    let source_build_name = build.name.clone();
    let profile = select_maxroll_profile(&build, profile_name)?;
    let localized = ImportLocalizedData::load(repo, &settings.selected_affix_language)
        .map_err(|error| error.to_string())?;
    let colors = ImportColors::from(settings);
    let lookups = &localized.maxroll;
    let tuning_prisms_by_id = load_affix_tuning_prisms(repo, &settings.selected_affix_language)?;
    let preset_name = if profile.name.trim().is_empty() {
        build.name.clone()
    } else {
        profile.name.clone()
    };

    let ctx = MaxrollItemContext {
        lookups,
        colors: &colors,
        tuning_prisms_by_id: &tuning_prisms_by_id,
    };
    let mut acc = MaxrollItemAccumulator {
        items: Vec::new(),
        affix_preset: AffixPreset {
            name: preset_name.clone(),
            affixes: Vec::new(),
        },
        warnings: Vec::new(),
        unmapped_entries: Vec::new(),
        aspect_snos: Vec::new(),
        unique_requirements: Vec::new(),
        rune_requirements: Vec::new(),
    };

    for (slot_id, item_ref) in &profile.items {
        import_maxroll_slot(
            &ctx,
            &mut acc,
            &build.data.items,
            &profile.name,
            *slot_id,
            item_ref,
        );
    }
    let MaxrollItemAccumulator {
        items,
        mut affix_preset,
        mut warnings,
        mut unmapped_entries,
        aspect_snos,
        unique_requirements,
        rune_requirements,
    } = acc;

    sort_and_dedup_affix_requirements(&mut affix_preset.affixes);

    let aspects =
        maxroll_aspect_requirements(&ctx, aspect_snos, &mut warnings, &mut unmapped_entries);
    let paragon_routes = maxroll_paragon_routes(profile, lookups);
    let paragon_boards = flatten_paragon_routes(&paragon_routes);

    let source_trace = vec![
        MAXROLL_LOAD_AVAILABLE_METHOD.to_string(),
        MAXROLL_CREATE_PRESET_METHOD.to_string(),
    ];
    let source_trace = if paragon_boards.is_empty() {
        source_trace
    } else {
        let mut trace = source_trace;
        trace.push("BuildsManagerMaxroll.CreatePresetFromMaxrollBuild(paragon)".to_string());
        trace
    };
    Ok(ImportWorkflowResult {
        provider: MAXROLL_PROVIDER.to_string(),
        source_url_or_file: source_path.display().to_string(),
        source_build_id,
        source_build_name,
        class_name: None,
        variant_name: Some(profile.name.clone()),
        affix_preset_name: preset_name.clone(),
        items,
        aspects: aspects.clone(),
        uniques: unique_requirements.clone(),
        runes: rune_requirements.clone(),
        affix_preset,
        non_affix_preset: NonAffixPreset {
            name: preset_name,
            powers: aspects
                .iter()
                .chain(unique_requirements.iter())
                .cloned()
                .collect::<Vec<_>>(),
            runes: rune_requirements,
            sigils: Vec::new(),
        },
        paragon_boards,
        paragon_routes,
        warnings,
        unmapped_entries,
        source_trace,
    })
}

/// Read-only inputs shared by the per-item import steps.
struct MaxrollItemContext<'a> {
    lookups: &'a MaxrollImportLookupTables,
    colors: &'a ImportColors,
    tuning_prisms_by_id: &'a HashMap<String, Vec<String>>,
}

/// What the item loop collects before the preset is assembled.
struct MaxrollItemAccumulator {
    items: Vec<ImportedItemAffixes>,
    affix_preset: AffixPreset,
    warnings: Vec<ImportWarning>,
    unmapped_entries: Vec<ImportUnmappedEntry>,
    aspect_snos: Vec<i32>,
    unique_requirements: Vec<NonAffixRequirement>,
    rune_requirements: Vec<NonAffixRequirement>,
}

/// Import one equipment slot of the selected profile into `acc`.
fn import_maxroll_slot(
    ctx: &MaxrollItemContext<'_>,
    acc: &mut MaxrollItemAccumulator,
    build_items: &BTreeMap<u32, MaxrollBuildItemFile>,
    profile_name: &str,
    slot_id: u32,
    item_ref: &u32,
) {
    let Some(item_type) = maxroll_item_type_from_slot(slot_id) else {
        push_import_warning(
            MAXROLL_PROVIDER,
            &mut acc.warnings,
            &mut acc.unmapped_entries,
            format!("Items[{slot_id}]"),
            item_ref.to_string(),
            // BuildsManagerMaxroll.cs:140 (`item.Key` is the slot id).
            format!("Imported Maxroll build contains unknown itemtype id: {slot_id}."),
            MAXROLL_CREATE_PRESET_METHOD,
        );
        return;
    };

    // Skip Charm and HoradricSeal. Not implemented yet.
    // Mirrors upstream `BuildsManagerMaxroll.ParseJsonProfile`: the
    // import continues silently without a `WarningOccurredMessage`.
    if item_type == "charm" || item_type == "horadricseal" {
        return;
    }

    let Some(item) = build_items.get(item_ref) else {
        push_import_warning(
            MAXROLL_PROVIDER,
            &mut acc.warnings,
            &mut acc.unmapped_entries,
            format!("profiles[{profile_name}].items[{slot_id}]"),
            item_ref.to_string(),
            format!("Imported Maxroll build contains missing item id: {item_ref}."),
            MAXROLL_CREATE_PRESET_METHOD,
        );
        return;
    };

    let unique_item = ctx.lookups.unique_item.match_candidate(&item.id);
    if let Some(unique_item) = &unique_item {
        acc.unique_requirements.push(NonAffixRequirement {
            id: unique_item.id.clone(),
            item_type: None,
            color: ctx.colors.uniques.clone(),
            kind: RuleNonAffixKind::Unique,
        });
    }

    let mut scope = MaxrollItemScope {
        ctx,
        item,
        item_type,
        item_ref,
        is_unique: unique_item.is_some(),
        affixes: Vec::new(),
    };
    scope.import_explicit_affixes(acc);
    if scope.is_unique {
        scope.import_implicit_affixes(acc);
    }
    scope.import_tempered_affixes(acc);

    for aspect in &item.aspects {
        if aspect.nid != 0 {
            acc.aspect_snos.push(aspect.nid);
        }
    }

    import_maxroll_rune_sockets(item, &mut acc.rune_requirements);

    if !scope.affixes.is_empty() {
        acc.items.push(ImportedItemAffixes {
            item_type: item_type.to_string(),
            affixes: scope.affixes,
        });
    }
}

/// One item of the build while its affixes are imported.
struct MaxrollItemScope<'a> {
    ctx: &'a MaxrollItemContext<'a>,
    item: &'a MaxrollBuildItemFile,
    item_type: &'static str,
    item_ref: &'a u32,
    is_unique: bool,
    /// The item's affix requirements, in import order.
    affixes: Vec<AffixRequirement>,
}

impl MaxrollItemScope<'_> {
    fn import_explicit_affixes(&mut self, acc: &mut MaxrollItemAccumulator) {
        let lookups = self.ctx.lookups;
        let colors = self.ctx.colors;
        for (index, explicit_affix) in self.item.explicits.iter().enumerate() {
            if !self.is_unique && index > 3 {
                break;
            }
            if self.is_unique && index > 4 {
                break;
            }

            let affix_sno = explicit_affix.nid.to_string();
            if let Some(affix_info) = lookups.affix.match_candidate_by_sno(&affix_sno) {
                let tuning_prisms =
                    affix_tuning_prisms(self.ctx.tuning_prisms_by_id, &affix_info.id);
                let requirement = AffixRequirement {
                    affix_id: affix_info.id,
                    label: Some(affix_info.label),
                    item_type: self.item_type.to_string(),
                    // Upstream `BuildsManagerMaxroll.cs:330`: greater picks
                    // the greater color, else the normal one.
                    color: if explicit_affix.greater {
                        colors.greater.clone()
                    } else {
                        colors.normal.clone()
                    },
                    is_any_type: false,
                    is_greater: explicit_affix.greater,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms,
                };
                if !acc.affix_preset.affixes.iter().any(|affix| {
                    affix.affix_id == requirement.affix_id
                        && affix.item_type == requirement.item_type
                        && affix.is_implicit == requirement.is_implicit
                        && affix.is_tempered == requirement.is_tempered
                }) {
                    acc.affix_preset.affixes.push(requirement.clone());
                }
                self.affixes.push(requirement);
            } else if lookups
                .unique_affix
                .match_candidate_by_sno(&affix_sno)
                .is_none()
            {
                push_import_warning(
                    MAXROLL_PROVIDER,
                    &mut acc.warnings,
                    &mut acc.unmapped_entries,
                    format!("Items[{}].Explicits[{index}]", self.item_ref),
                    affix_sno.clone(),
                    // BuildsManagerMaxroll.cs:318.
                    format!("Imported Maxroll build contains unknown affix sno: {affix_sno}."),
                    MAXROLL_CREATE_PRESET_METHOD,
                );
            }
        }
    }

    fn import_implicit_affixes(&mut self, acc: &mut MaxrollItemAccumulator) {
        for (index, implicit_affix) in self.item.implicits.iter().enumerate() {
            let affix_sno = implicit_affix.nid.to_string();
            let Some(affix_info) = self.ctx.lookups.affix.match_candidate_by_sno(&affix_sno) else {
                push_import_warning(
                    MAXROLL_PROVIDER,
                    &mut acc.warnings,
                    &mut acc.unmapped_entries,
                    format!("Items[{}].Implicits[{index}]", self.item_ref),
                    affix_sno.clone(),
                    // BuildsManagerMaxroll.cs:203.
                    format!(
                        "Imported Maxroll build contains unknown implicit affix sno: {affix_sno}."
                    ),
                    MAXROLL_CREATE_PRESET_METHOD,
                );
                continue;
            };

            let tuning_prisms = affix_tuning_prisms(self.ctx.tuning_prisms_by_id, &affix_info.id);
            let requirement = AffixRequirement {
                affix_id: affix_info.id,
                label: Some(affix_info.label),
                item_type: self.item_type.to_string(),
                // Upstream `BuildsManagerMaxroll.cs:214,262,287`: uniques'
                // implicit affixes take the implicit color.
                color: self.ctx.colors.implicit.clone(),
                is_any_type: false,
                is_greater: false,
                is_implicit: true,
                is_tempered: false,
                tuning_prisms,
            };
            if !acc.affix_preset.affixes.iter().any(|affix| {
                affix.affix_id == requirement.affix_id
                    && affix.item_type == requirement.item_type
                    && affix.is_implicit
            }) {
                acc.affix_preset.affixes.push(requirement.clone());
            }
            self.affixes.push(requirement);
        }
    }

    fn import_tempered_affixes(&mut self, acc: &mut MaxrollItemAccumulator) {
        for (index, tempered_affix) in self.item.tempered.iter().enumerate() {
            let affix_sno = tempered_affix.nid.to_string();
            let Some(affix_info) = self.ctx.lookups.affix.match_candidate_by_sno(&affix_sno) else {
                push_import_warning(
                    MAXROLL_PROVIDER,
                    &mut acc.warnings,
                    &mut acc.unmapped_entries,
                    format!("Items[{}].Tempered[{index}]", self.item_ref),
                    affix_sno.clone(),
                    // BuildsManagerMaxroll.cs:349.
                    format!(
                        "Imported Maxroll build contains unknown tempered affix sno: {affix_sno}."
                    ),
                    MAXROLL_CREATE_PRESET_METHOD,
                );
                continue;
            };

            let tuning_prisms = affix_tuning_prisms(self.ctx.tuning_prisms_by_id, &affix_info.id);
            let requirement = AffixRequirement {
                affix_id: affix_info.id,
                label: Some(affix_info.label),
                item_type: self.item_type.to_string(),
                // Upstream `BuildsManagerMaxroll.cs:360`: tempered affixes
                // take the tempered color.
                color: self.ctx.colors.tempered.clone(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms,
            };
            if !acc.affix_preset.affixes.iter().any(|affix| {
                affix.affix_id == requirement.affix_id
                    && affix.item_type == requirement.item_type
                    && affix.is_tempered
            }) {
                acc.affix_preset.affixes.push(requirement.clone());
            }
            self.affixes.push(requirement);
        }
    }
}

/// Collect the item's runed sockets as rune requirements, once per rune id.
fn import_maxroll_rune_sockets(
    item: &MaxrollBuildItemFile,
    rune_requirements: &mut Vec<NonAffixRequirement>,
) {
    for socket in &item.sockets {
        let rune_id = socket.clone().unwrap_or_default();
        if !rune_id
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Rune_"))
        {
            continue;
        }
        let rune_id = format!("Item_{rune_id}");
        if !rune_requirements
            .iter()
            .any(|entry: &NonAffixRequirement| entry.id == rune_id)
        {
            rune_requirements.push(NonAffixRequirement {
                id: rune_id,
                item_type: None,
                // Upstream assigns no color to Maxroll runes
                // (`BuildsManagerMaxroll.cs:159`): the `ItemAffix.Color`
                // default stays.
                color: UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR.to_string(),
                kind: RuleNonAffixKind::Rune,
            });
        }
    }
}

/// Resolve the collected aspect snos against the aspect catalog, one
/// requirement per equipment item type.
fn maxroll_aspect_requirements(
    ctx: &MaxrollItemContext<'_>,
    aspect_snos: Vec<i32>,
    warnings: &mut Vec<ImportWarning>,
    unmapped_entries: &mut Vec<ImportUnmappedEntry>,
) -> Vec<NonAffixRequirement> {
    let mut aspects = Vec::new();
    for (index, aspect_sno) in aspect_snos.into_iter().enumerate() {
        match ctx
            .lookups
            .aspect
            .match_candidate_by_sno(&aspect_sno.to_string())
        {
            Some(candidate) => {
                for item_type in EQUIPMENT_ITEM_TYPES {
                    aspects.push(NonAffixRequirement {
                        id: candidate.id.clone(),
                        item_type: Some((*item_type).to_string()),
                        color: ctx.colors.aspects.clone(),
                        kind: RuleNonAffixKind::Aspect,
                    });
                }
            }
            None => push_import_warning(
                MAXROLL_PROVIDER,
                warnings,
                unmapped_entries,
                format!("Aspect[{index}]"),
                aspect_sno.to_string(),
                // BuildsManagerMaxroll.cs:388.
                format!("Imported Maxroll build contains unknown aspect sno: {aspect_sno}."),
                MAXROLL_CREATE_PRESET_METHOD,
            ),
        }
    }
    aspects
}

/// The paragon routes of the selected profile, one per step.
fn maxroll_paragon_routes(
    profile: &MaxrollBuildProfileFile,
    lookups: &MaxrollImportLookupTables,
) -> Vec<ImportedParagonRoute> {
    profile
        .paragon
        .steps
        .iter()
        .map(|step| ImportedParagonRoute {
            step_name: Some(step.name.clone()),
            boards: step
                .data
                .iter()
                .map(|board| {
                    let mut active_nodes = Vec::new();
                    for location in board.nodes.keys() {
                        active_nodes
                            .push(maxroll_rotate_paragon_location(*location, board.rotation));
                    }
                    active_nodes.sort_unstable();
                    ImportedParagonBoard {
                        index: 0,
                        name: lookups.board_name(&board.id),
                        glyph: lookups.glyph_name(&board.glyph),
                        rotation: match board.rotation {
                            0 => "0°",
                            1 => "90°",
                            2 => "180°",
                            3 => "270°",
                            _ => "?°",
                        }
                        .to_string(),
                        active_nodes,
                    }
                })
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>()
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildFile {
    data: String,
    date: String,
    id: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildDataFile {
    #[serde(default)]
    profiles: Vec<MaxrollBuildProfileFile>,
    #[serde(default)]
    items: BTreeMap<u32, MaxrollBuildItemFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildProfileFile {
    name: String,
    #[serde(default)]
    items: BTreeMap<u32, u32>,
    #[serde(default)]
    paragon: MaxrollBuildParagonFile,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct MaxrollBuildParagonFile {
    #[serde(default)]
    steps: Vec<MaxrollBuildParagonStepFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildParagonStepFile {
    name: String,
    #[serde(default)]
    data: Vec<MaxrollBuildParagonStepDataFile>,
}

/// Paragon step-data entry (`MaxrollBuildDataParagonStepData`).
///
/// `id`, `rotation` and `glyph` all carry a default, mirroring upstream:
/// System.Text.Json leaves the C# property initializers (`= string.Empty`)
/// in place when the property is absent and `int` stays `0`, so a live
/// planner envelope that drops these properties still imports (build
/// `7vu83u0t` ships paragon step-data without `glyph`).
#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildParagonStepDataFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    nodes: BTreeMap<usize, i32>,
    #[serde(default)]
    rotation: i32,
    #[serde(default)]
    glyph: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildItemFile {
    id: String,
    #[serde(default)]
    explicits: Vec<MaxrollBuildItemAffixFile>,
    #[serde(default)]
    implicits: Vec<MaxrollBuildItemAffixFile>,
    #[serde(default)]
    tempered: Vec<MaxrollBuildItemAffixFile>,
    #[serde(default)]
    aspects: Vec<MaxrollBuildItemAspectFile>,
    #[serde(default)]
    sockets: Vec<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildItemAffixFile {
    nid: i32,
    #[serde(default)]
    greater: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct MaxrollBuildItemAspectFile {
    nid: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct MaxrollImportLookupTables {
    pub(crate) affix: ImportSnoLookupIndex,
    aspect: ImportSnoLookupIndex,
    unique_affix: ImportSnoLookupIndex,
    unique_item: MaxrollUniqueItemIndex,
    board: ImportExactIdLookupIndex,
    glyph: ImportExactIdLookupIndex,
}

impl MaxrollImportLookupTables {
    pub(crate) fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Ok(Self {
            affix: load_sno_lookup_index(repo, language, "Affixes", "DescriptionClean")?,
            aspect: load_sno_lookup_index(repo, language, "Aspects", "Name")?,
            unique_affix: load_sno_lookup_index(repo, language, "Uniques", "Name")?,
            unique_item: load_maxroll_unique_item_index(repo, language)?,
            board: load_exact_id_lookup_index(repo, language, "ParagonBoards")?,
            glyph: load_exact_id_lookup_index(repo, language, "ParagonGlyphs")?,
        })
    }

    fn board_name(&self, id: &str) -> String {
        self.board.match_id(id).unwrap_or_else(|| id.to_string())
    }

    fn glyph_name(&self, id: &str) -> String {
        self.glyph.match_id(id).unwrap_or_else(|| id.to_string())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImportSnoLookupIndex {
    candidates: Vec<ImportSnoLookupCandidate>,
}

impl ImportSnoLookupIndex {
    pub(crate) fn match_candidate_by_sno(&self, sno: &str) -> Option<ImportSnoLookupCandidate> {
        self.candidates
            .iter()
            .find(|candidate| {
                candidate
                    .snos
                    .iter()
                    .any(|candidate_sno| candidate_sno == sno)
            })
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImportSnoLookupCandidate {
    pub(crate) id: String,
    label: String,
    snos: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MaxrollUniqueItemIndex {
    candidates: Vec<MaxrollUniqueItemCandidate>,
}

impl MaxrollUniqueItemIndex {
    pub(crate) fn match_candidate(&self, item_id: &str) -> Option<MaxrollUniqueItemCandidate> {
        self.candidates
            .iter()
            .find(|candidate| {
                candidate
                    .item_ids
                    .iter()
                    .any(|candidate_id| candidate_id.contains(item_id))
                    || candidate.actor_id.as_deref() == Some(item_id)
            })
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MaxrollUniqueItemCandidate {
    pub(crate) id: String,
    item_ids: Vec<String>,
    actor_id: Option<String>,
}

fn parse_maxroll_build_file(path: &Path) -> Result<MaxrollBuildWrapper, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let outer: MaxrollBuildFile = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let data: MaxrollBuildDataFile = serde_json::from_str(&outer.data)
        .map_err(|error| format!("failed to parse {} data payload: {error}", path.display()))?;
    Ok(MaxrollBuildWrapper {
        data,
        id: outer.id,
        name: outer.name,
        date: outer.date,
    })
}

#[derive(Debug, Clone)]
struct MaxrollBuildWrapper {
    data: MaxrollBuildDataFile,
    id: String,
    name: String,
    /// The envelope `date` field (`MaxrollBuildJson.cs`), the build row's
    /// provenance stamp.
    date: String,
}

fn select_maxroll_profile<'a>(
    build: &'a MaxrollBuildWrapper,
    profile_name: Option<&str>,
) -> Result<&'a MaxrollBuildProfileFile, String> {
    if let Some(profile_name) = profile_name {
        return build
            .data
            .profiles
            .iter()
            .find(|profile| profile.name == profile_name)
            .ok_or_else(|| {
                format!(
                    "missing Maxroll profile '{profile_name}' in build '{}'",
                    build.id
                )
            });
    }
    build
        .data
        .profiles
        .first()
        .ok_or_else(|| format!("build '{}' has no profiles", build.id))
}

/// SNO-keyed provider lookup (Maxroll `Items[..].Explicits[..].Nid`).
///
/// The resolved id is the canonical identity of the pinned catalog row — the
/// complete `IdName` — never a shortened `IdNameList` entry: upstream
/// `AffixManager.GetAffixInfoMaxrollByIdSno` returns the row and the caller
/// stores `affixInfo.IdName` (BuildsManagerMaxroll.cs:195-217).
fn load_sno_lookup_index(
    repo: &AssetRepository,
    language: &str,
    category: &str,
    label_field: &str,
) -> Result<ImportSnoLookupIndex, String> {
    let catalog = IdentityCatalog::load(repo, language, category, label_field)?;
    let mut candidates = Vec::new();
    for row in catalog.rows() {
        let Some(label) = row.label.clone() else {
            continue;
        };
        if row.snos.is_empty() {
            continue;
        }
        candidates.push(ImportSnoLookupCandidate {
            id: row.id_name.clone(),
            label,
            snos: row.snos.clone(),
        });
    }
    Ok(ImportSnoLookupIndex { candidates })
}

fn load_maxroll_unique_item_index(
    repo: &AssetRepository,
    language: &str,
) -> Result<MaxrollUniqueItemIndex, String> {
    let path = repo.data_dir().join(format!("Uniques.{language}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let mut candidates = Vec::new();
    for item in items {
        let Some(id) = string_field(&item, "IdName") else {
            continue;
        };
        let mut item_ids = Vec::new();
        if let Some(values) = item
            .get("IdNameItemList")
            .and_then(|value| value.as_array())
        {
            for value in values {
                if let Some(item_id) = value.as_str() {
                    item_ids.push(item_id.to_string());
                }
            }
        } else if let Some(item_ids_field) = string_field(&item, "IdNameItem") {
            item_ids.extend(item_ids_field.split(';').map(ToOwned::to_owned));
        }
        let actor_id = string_field(&item, "IdNameItemActor");
        if !item_ids.is_empty() || actor_id.is_some() {
            candidates.push(MaxrollUniqueItemCandidate {
                id,
                item_ids,
                actor_id,
            });
        }
    }
    Ok(MaxrollUniqueItemIndex { candidates })
}

pub(crate) fn load_maxroll_unique_item_index_checked(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
) -> Result<MaxrollUniqueItemIndex, ImportLocalizedDataError> {
    ensure_localized_data_file(repo, language, component)?;
    load_maxroll_unique_item_index(repo, language)
        .map_err(|message| typed_localized_data_error(repo, language, component, message))
}

fn maxroll_item_type_from_slot(slot: u32) -> Option<&'static str> {
    match slot {
        4 => Some("helm"),
        5 => Some("chest"),
        6 => Some("offhand"),
        7 | 8 | 9 | 11 | 12 => Some("weapon"),
        10 => Some("ranged"),
        13 => Some("gloves"),
        14 => Some("pants"),
        15 => Some("boots"),
        16 | 17 => Some("ring"),
        18 => Some("amulet"),
        20 => Some("horadricseal"), // Talisman Seal
        21..=26 => Some("charm"),   // Talisman Charm
        _ => None,
    }
}

pub(crate) fn maxroll_rotate_paragon_location(location: usize, rotation: i32) -> usize {
    let location_x = location % 21;
    let location_y = location / 21;
    match rotation {
        0 => location,
        1 => {
            let mut location_xt = 21 - location_y;
            let location_yt = location_x;
            location_xt -= 1;
            location_yt * 21 + location_xt
        }
        2 => {
            let mut location_xt = 21 - location_x;
            let mut location_yt = 21 - location_y;
            location_xt -= 1;
            location_yt -= 1;
            location_yt * 21 + location_xt
        }
        3 => {
            let location_xt = location_y;
            let mut location_yt = 21 - location_x;
            location_yt -= 1;
            location_yt * 21 + location_xt
        }
        _ => location,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
