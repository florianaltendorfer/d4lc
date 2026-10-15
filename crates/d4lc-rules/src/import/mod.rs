mod d2core;
mod d4builds;
mod infinity_builds;
mod maxroll;
mod mobalytics;

pub use d2core::{
    import_d2core_build_variant, load_available_d2core_builds, load_available_d2core_variants,
};
pub use d4builds::{
    convert_d4builds_site_variants, export_d4builds_build_cache, import_d4builds_build_variant,
    load_available_d4builds_builds, load_available_d4builds_variants,
    parse_d4builds_site_paragon_boards, D4BuildsAffixFile, D4BuildsBuildFile,
    D4BuildsConversionLookups, D4BuildsConvertedBuild, D4BuildsConvertedVariant,
    D4BuildsParagonBoardFile, D4BuildsSiteAffix, D4BuildsSiteAffixGroup, D4BuildsSiteBuild,
    D4BuildsSiteParagonBoard, D4BuildsSiteVariant, D4BuildsVariantFile,
};
pub use infinity_builds::{
    export_infinity_builds_build_cache, extract_infinity_builds_share_script,
    extract_infinity_builds_share_scripts, import_infinity_builds_build_variant,
    import_infinity_builds_share_html, load_available_infinity_builds_builds,
    load_available_infinity_builds_variants, InfinityBuildsAffix, InfinityBuildsBuild,
    InfinityBuildsBuildVariant, InfinityBuildsShareImport,
};
pub use maxroll::{
    import_maxroll_build_profile, load_available_maxroll_builds, load_available_maxroll_variants,
};
pub use mobalytics::{
    convert_mobalytics_graphql_build, export_mobalytics_build_cache,
    export_mobalytics_profile_cache, import_mobalytics_build_variant,
    load_available_mobalytics_builds, load_available_mobalytics_profiles,
    load_available_mobalytics_variants, parse_mobalytics_graphql_build,
    parse_mobalytics_graphql_build_with_warnings, parse_mobalytics_profile_state,
    remove_mobalytics_profile, MobalyticsAffixEntry, MobalyticsBuildFile,
    MobalyticsConversionLookups, MobalyticsConvertedBuild, MobalyticsConvertedVariant,
    MobalyticsParagonBoardFile, MobalyticsProfileFile, MobalyticsProfileVariant,
    MobalyticsVariantFile, MOBALYTICS_SHOW_BROWSER_DEFAULT,
};

use crate::import::d2core::D2CoreParagonLookup;
use crate::import::maxroll::{
    load_maxroll_unique_item_index_checked, MaxrollImportLookupTables, MaxrollUniqueItemIndex,
};
use crate::model::{AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement};
use d4lc_assets::catalog::IdentityCatalog;
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWarning {
    pub provider: String,
    pub source_field: String,
    pub source_value: String,
    pub message: String,
    pub upstream_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportUnmappedEntry {
    pub source_field: String,
    pub source_value: String,
    pub message: String,
    pub upstream_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportVariantDescriptor {
    pub source_build_id: String,
    pub source_build_name: String,
    pub variant_name: Option<String>,
}

/// One stored provider build as the tabbed build-import dialog lists it —
/// the Linux-port analog of an upstream `D2CoreBuild` / `D4BuildsBuild` /
/// `InfinityBuildsBuild` / `MaxrollBuild` / `MobalyticsBuild` row
/// (`{Name} ({Date})` tooltip plus the variant list on the right).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportBuildDescriptor {
    /// The provider build id (upstream `Id`): also the `{id}.json` file
    /// stem under `Builds/{Provider}`.
    pub id: String,
    /// The provider build name (upstream `Name`).
    pub name: String,
    /// The provider build URL when the stored entity carries one
    /// (upstream `Url` — InfinityBuilds update re-downloads by `Url`,
    /// Mobalytics stores the page URL verbatim). Empty otherwise.
    pub url: String,
    /// The build's provenance date, provider-specific (upstream `Date`):
    /// D2Core `_updateTime` (unix ms, rendered UTC), D4Builds scraped
    /// "last updated" text, InfinityBuilds cached `UpdatedAt`, Maxroll
    /// envelope `date`, Mobalytics document `updatedAt`. Empty when the
    /// stored file carries no date.
    pub date: String,
    /// The variant / profile names of the build (upstream
    /// `Data.Variants` / `Variants` / `Data.Profiles`).
    pub variant_names: Vec<String>,
    /// The JSON file this row was parsed from. Import flows use this
    /// path directly so a build whose file stem does not mirror its id
    /// still imports its own JSON (the loader knows the file it read);
    /// empty when the row carries no file (profile summaries).
    pub source_path: String,
}

/// One stored-provider builds-list load: the parsed rows plus the
/// structured warnings for files that were skipped instead of failing the
/// whole list. Mirrors upstream's stored-builds enumeration, where one
/// malformed cached file must not take the list down
/// (`BuildsManagerMaxroll.LoadAvailableMaxrollBuilds` wraps its
/// enumeration in a try/catch that logs and keeps the app usable).
#[derive(Debug, Clone, Default)]
pub struct ImportBuildList {
    /// The build rows parsed from the directory (skipping bad files).
    pub builds: Vec<ImportBuildDescriptor>,
    /// One structured warning per skipped file, naming it
    /// (`source_field`) with the underlying parse error (`message`).
    pub warnings: Vec<ImportWarning>,
}

/// Format a unix-epoch-milliseconds timestamp as `YYYY-MM-DD HH:MM` (UTC).
///
/// Upstream renders the D2Core `_updateTime` through WPF's local-time
/// conversion. The port carries no timezone database, so the stamp renders
/// in UTC: same provenance, stable rendering, only the timezone differs.
/// `0` (the serde default for builds without `_updateTime`) yields an empty
/// string so the tooltip degrades to `{Name}`-free `{Name}` form.
/// Also the D4Builds scrape fallback for a missing
/// `builder__last__updated` element (upstream falls back to
/// `DateTime.Now`).
pub fn unix_ms_to_utc_timestamp(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let seconds = ms / 1000;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Days since 1970-01-01 to a proleptic-Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedItemAffixes {
    pub item_type: String,
    pub affixes: Vec<AffixRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedParagonBoard {
    pub name: String,
    pub glyph: String,
    pub rotation: String,
    pub active_nodes: Vec<usize>,
    /// Upstream `ParagonBoard.Index`: set by the D2Core import only (its
    /// board ordering value, `BuildsManagerD2Core.cs:462`), `0` elsewhere.
    #[serde(default, skip_serializing_if = "index_is_default")]
    pub index: i32,
}

fn index_is_default(index: &i32) -> bool {
    *index == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedParagonRoute {
    pub step_name: Option<String>,
    pub boards: Vec<ImportedParagonBoard>,
}

/// The seven upstream `DefaultColor*` settings an import paints its
/// entries with (`SettingsD4.cs:18-24`, projected from
/// [`AppSettings::default_colors`]).
///
/// Every provider conversion reads the configured class color instead of a
/// fixed import color, exactly like the upstream `Convert*` methods
/// (`BuildsManagerD4Builds.cs:548-558`, `BuildsManagerD2Core.cs:403-412`,
/// `BuildsManagerInfinityBuilds.cs:647-650`, `BuildsManagerMobalytics.cs:
/// 680-683`), so an imported preset stores the colors the overlay would
/// draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportColors {
    pub greater: String,
    pub implicit: String,
    pub normal: String,
    pub tempered: String,
    pub aspects: String,
    pub uniques: String,
    pub runes: String,
}

impl Default for ImportColors {
    /// The upstream defaults: every class defaults to `Colors.Green`
    /// (`SettingsD4.cs:18-24`).
    fn default() -> Self {
        let green = || UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR.to_string();
        Self {
            greater: green(),
            implicit: green(),
            normal: green(),
            tempered: green(),
            aspects: green(),
            uniques: green(),
            runes: green(),
        }
    }
}

impl From<&AppSettings> for ImportColors {
    fn from(settings: &AppSettings) -> Self {
        let colors = &settings.default_colors;
        Self {
            greater: colors.greater.clone(),
            implicit: colors.implicit.clone(),
            normal: colors.normal.clone(),
            tempered: colors.tempered.clone(),
            aspects: colors.aspects.clone(),
            uniques: colors.uniques.clone(),
            runes: colors.runes.clone(),
        }
    }
}

impl ImportColors {
    /// Upstream `BuildsManagerInfinityBuilds.ConvertItemAffix` (:647-650)
    /// and `BuildsManagerMobalytics.ConvertItemAffix` (:680-683): the
    /// implicit flag wins, then greater, then tempered, else normal.
    fn by_implicit_greater_tempered(
        &self,
        is_implicit: bool,
        is_greater: bool,
        is_tempered: bool,
    ) -> String {
        if is_implicit {
            self.implicit.clone()
        } else if is_greater {
            self.greater.clone()
        } else if is_tempered {
            self.tempered.clone()
        } else {
            self.normal.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWorkflowResult {
    pub provider: String,
    pub source_url_or_file: String,
    pub source_build_id: String,
    pub source_build_name: String,
    pub class_name: Option<String>,
    pub variant_name: Option<String>,
    pub affix_preset_name: String,
    pub items: Vec<ImportedItemAffixes>,
    pub aspects: Vec<NonAffixRequirement>,
    pub uniques: Vec<NonAffixRequirement>,
    pub runes: Vec<NonAffixRequirement>,
    pub affix_preset: AffixPreset,
    pub non_affix_preset: NonAffixPreset,
    pub paragon_boards: Vec<ImportedParagonBoard>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragon_routes: Vec<ImportedParagonRoute>,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
    pub source_trace: Vec<String>,
}

/// Upstream `ItemAffix.Color` default
/// (`D4Companion.Entities/ItemAffix.cs:15` = `Colors.Green`). The
/// conversions whose upstream source assigns no color at all keep this
/// default instead of a configured class color: D2Core runes and aspects
/// (`BuildsManagerD2Core.cs:362-364,432-441`) and Maxroll runes
/// (`BuildsManagerMaxroll.cs:159`).
const UPSTREAM_ITEM_AFFIX_DEFAULT_COLOR: &str = "Green";

const EQUIPMENT_ITEM_TYPES: &[&str] = &[
    "helm", "chest", "gloves", "pants", "boots", "amulet", "ring", "weapon", "ranged", "offhand",
];

const D4BUILDS_AFFIX_THRESHOLD: f64 = 90.0;

const D4BUILDS_NAME_THRESHOLD: f64 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalizedDataComponent {
    ItemTypes,
    Affixes,
    Aspects,
    Uniques,
    Runes,
    Sigils,
    ParagonBoards,
    ParagonGlyphs,
}

impl LocalizedDataComponent {
    const fn file_stem(self) -> &'static str {
        match self {
            Self::ItemTypes => "ItemTypes",
            Self::Affixes => "Affixes",
            Self::Aspects => "Aspects",
            Self::Uniques => "Uniques",
            Self::Runes => "Runes",
            Self::Sigils => "Sigils",
            Self::ParagonBoards => "ParagonBoards",
            Self::ParagonGlyphs => "ParagonGlyphs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLocalizedDataError {
    pub language: String,
    pub component: LocalizedDataComponent,
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ImportLocalizedDataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} [{} {}]: {}",
            self.message,
            self.component.file_stem(),
            self.language,
            self.path
        )
    }
}

impl std::error::Error for ImportLocalizedDataError {}

#[derive(Debug, Clone)]
pub struct ImportLocalizedData {
    language: String,
    item_type_count: usize,
    affix_count: usize,
    aspect_count: usize,
    unique_count: usize,
    rune_count: usize,
    sigil_count: usize,
    maxroll: MaxrollImportLookupTables,
    d2core_affixes: ImportIdNameLookupIndex,
    d2core_uniques: MaxrollUniqueItemIndex,
    d2core_paragon: D2CoreParagonLookup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLocalizedDataState {
    pub language: String,
    pub item_type_count: usize,
    pub affix_count: usize,
    pub aspect_count: usize,
    pub unique_count: usize,
    pub rune_count: usize,
    pub sigil_count: usize,
}

impl ImportLocalizedData {
    pub fn load(repo: &AssetRepository, language: &str) -> Result<Self, ImportLocalizedDataError> {
        let affix_count = count_localized_rows(repo, language, LocalizedDataComponent::Affixes)?;
        let aspect_count = count_localized_rows(repo, language, LocalizedDataComponent::Aspects)?;
        let unique_count = count_localized_rows(repo, language, LocalizedDataComponent::Uniques)?;
        let rune_count = count_localized_rows(repo, language, LocalizedDataComponent::Runes)?;
        let sigil_count = count_localized_rows(repo, language, LocalizedDataComponent::Sigils)?;
        let item_type_count =
            count_localized_rows(repo, language, LocalizedDataComponent::ItemTypes)?;
        ensure_localized_data_file(repo, language, LocalizedDataComponent::ParagonBoards)?;
        ensure_localized_data_file(repo, language, LocalizedDataComponent::ParagonGlyphs)?;

        Ok(Self {
            language: language.to_string(),
            item_type_count,
            affix_count,
            aspect_count,
            unique_count,
            rune_count,
            sigil_count,
            maxroll: MaxrollImportLookupTables::load(repo, language).map_err(|message| {
                typed_localized_data_error(
                    repo,
                    language,
                    LocalizedDataComponent::ParagonBoards,
                    message,
                )
            })?,
            d2core_affixes: load_id_name_lookup_index_checked(
                repo,
                language,
                LocalizedDataComponent::Affixes,
                "DescriptionClean",
            )?,
            d2core_uniques: load_maxroll_unique_item_index_checked(
                repo,
                language,
                LocalizedDataComponent::Uniques,
            )?,
            d2core_paragon: D2CoreParagonLookup::load(repo, language).map_err(|message| {
                typed_localized_data_error(
                    repo,
                    language,
                    LocalizedDataComponent::ParagonBoards,
                    message,
                )
            })?,
        })
    }

    pub fn switch_language(
        &mut self,
        repo: &AssetRepository,
        language: &str,
    ) -> Result<(), ImportLocalizedDataError> {
        if self.language == language {
            return Ok(());
        }

        *self = Self::load(repo, language)?;
        Ok(())
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn affix_label(&self, id: &str) -> Option<String> {
        let _ = id;
        None
    }

    pub fn state(&self) -> ImportLocalizedDataState {
        ImportLocalizedDataState {
            language: self.language.clone(),
            item_type_count: self.item_type_count,
            affix_count: self.affix_count,
            aspect_count: self.aspect_count,
            unique_count: self.unique_count,
            rune_count: self.rune_count,
            sigil_count: self.sigil_count,
        }
    }
}

/// True for the raw browser-capture cache files (`{id}.raw.json`): the
/// interim D4Builds capture (removed) and the Mobalytics capture still
/// write that suffix, and no build loader may mistake them for stored
/// builds.
fn is_raw_capture_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".raw.json"))
}

/// Upstream's rotation label for a quarter-turn count.
fn paragon_rotation_text(rotation: i64) -> &'static str {
    match rotation {
        0 => "0°",
        1 => "90°",
        2 => "180°",
        3 => "270°",
        _ => "?°",
    }
}

/// Upstream's rotation label for a rotation in degrees ("0°" / "90°" /
/// "180°" / "270°" / "?°").
fn degree_rotation_text(rotation_degrees: i32) -> &'static str {
    match rotation_degrees {
        0 => "0°",
        90 => "90°",
        180 => "180°",
        270 => "270°",
        _ => "?°",
    }
}

/// Cache-file safety gate shared by every build cache writer and remover:
/// the build id becomes the `{id}.json` file name, so only ASCII
/// alphanumerics and `-_.~` are accepted, and `.` / `..` / empty are
/// refused. Upstream derives the same file names from its id gates; the
/// gates that do not pin a charset (D2Core's 4-character gate, Mobalytics'
/// slug extraction, ids read back from stored JSON) rely on this check.
pub fn ensure_cache_filename_safe(id: &str) -> Result<(), String> {
    let safe = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'));
    if safe {
        Ok(())
    } else {
        Err(format!(
            "build id '{id}' cannot be used as a cache file name"
        ))
    }
}

#[derive(Debug, Clone)]
struct ImportExactIdLookupIndex {
    candidates: Vec<ImportExactIdLookupCandidate>,
}

impl ImportExactIdLookupIndex {
    fn match_id(&self, id: &str) -> Option<String> {
        self.candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .map(|candidate| candidate.label.clone())
    }

    fn match_id_case_insensitive(&self, id: &str) -> Option<String> {
        self.candidates
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(id))
            .map(|candidate| candidate.label.clone())
    }

    fn match_id_contains(&self, fragment: &str) -> Option<String> {
        self.candidates
            .iter()
            .find(|candidate| candidate.id.contains(fragment))
            .map(|candidate| candidate.label.clone())
    }
}

#[derive(Debug, Clone)]
struct ImportExactIdLookupCandidate {
    id: String,
    label: String,
}

#[derive(Debug, Clone)]
struct ImportIdNameLookupIndex {
    candidates: Vec<ImportLookupCandidate>,
}

impl ImportIdNameLookupIndex {
    fn match_id_name(&self, raw_text: &str) -> Option<ImportLookupCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.normalized_label == raw_text)
            .cloned()
    }
}

fn flatten_paragon_routes(routes: &[ImportedParagonRoute]) -> Vec<ImportedParagonBoard> {
    routes
        .iter()
        .flat_map(|route| route.boards.iter().cloned())
        .collect()
}

fn load_exact_id_lookup_index(
    repo: &AssetRepository,
    language: &str,
    category: &str,
) -> Result<ImportExactIdLookupIndex, String> {
    let path = repo.data_dir().join(format!("{category}.{language}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let mut candidates = Vec::new();
    for item in items {
        let Some(id) = string_field(&item, "IdName") else {
            continue;
        };
        let Some(label) = string_field(&item, "Name") else {
            continue;
        };
        candidates.push(ImportExactIdLookupCandidate { id, label });
    }
    Ok(ImportExactIdLookupIndex { candidates })
}

/// `IdNameList`-keyed provider lookup (D2Core `Mods[..].Name`).
///
/// The `IdNameList` entries are the lookup keys of the pinned catalog row and
/// the resolved id is that row's complete `IdName`: upstream
/// `AffixManager.GetAffixInfoByIdName` is
/// `_affixes.FirstOrDefault(a => a.IdNameList.Contains(affixIdName))` and the
/// caller stores `affixInfo.IdName` (BuildsManagerD2Core.cs:390-412).
fn load_id_name_lookup_index(
    repo: &AssetRepository,
    language: &str,
    category: &str,
    label_field: &str,
) -> Result<ImportIdNameLookupIndex, String> {
    let catalog = IdentityCatalog::load(repo, language, category, label_field)?;
    let mut candidates = Vec::new();
    for row in catalog.rows() {
        let Some(label) = row.label.clone() else {
            continue;
        };
        for alias in &row.aliases {
            candidates.push(ImportLookupCandidate {
                id: row.id_name.clone(),
                label: label.clone(),
                normalized_label: alias.clone(),
            });
        }
    }
    Ok(ImportIdNameLookupIndex { candidates })
}

fn load_id_name_lookup_index_checked(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
    label_field: &str,
) -> Result<ImportIdNameLookupIndex, ImportLocalizedDataError> {
    ensure_localized_data_file(repo, language, component)?;
    load_id_name_lookup_index(repo, language, component.file_stem(), label_field)
        .map_err(|message| typed_localized_data_error(repo, language, component, message))
}

#[derive(Debug, Clone)]
struct ImportLookupIndex {
    candidates: Vec<ImportLookupCandidate>,
    scorer: ImportLookupScorer,
    threshold: f64,
}

impl ImportLookupIndex {
    fn match_candidate(&self, raw_text: &str) -> Option<ImportLookupCandidate> {
        self.match_candidate_with_score(raw_text)
            .map(|(candidate, _)| candidate)
    }

    fn match_candidate_with_score(&self, raw_text: &str) -> Option<(ImportLookupCandidate, f64)> {
        let normalized = normalize_import_text(raw_text);
        let mut winner: Option<(&ImportLookupCandidate, f64)> = None;
        for candidate in &self.candidates {
            let current = score_import_text(self.scorer, &normalized, &candidate.normalized_label);
            let is_better = winner.is_none_or(|(_, best_score)| current > best_score);
            if is_better {
                winner = Some((candidate, current));
            }
        }
        winner
            .filter(|(_, score)| *score >= self.threshold)
            .map(|(candidate, score)| (candidate.clone(), score))
    }
}

#[derive(Debug, Clone)]
struct ImportLookupCandidate {
    id: String,
    label: String,
    normalized_label: String,
}

#[derive(Debug, Clone, Copy)]
enum ImportLookupScorer {
    Ratio,
    TokenSet,
}

fn sorted_json_files(builds_dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries = fs::read_dir(builds_dir)
        .map_err(|error| format!("failed to read {}: {error}", builds_dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("failed to read directory entry: {error}"))?
            .path();
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn push_import_warning(
    provider: &str,
    warnings: &mut Vec<ImportWarning>,
    unmapped_entries: &mut Vec<ImportUnmappedEntry>,
    source_field: String,
    source_value: String,
    message: String,
    upstream_method: &str,
) {
    warnings.push(ImportWarning {
        provider: provider.to_string(),
        source_field: source_field.clone(),
        source_value: source_value.clone(),
        message: message.clone(),
        upstream_method: upstream_method.to_string(),
    });
    unmapped_entries.push(ImportUnmappedEntry {
        source_field,
        source_value,
        message,
        upstream_method: upstream_method.to_string(),
    });
}

fn sort_and_dedup_affix_requirements(requirements: &mut Vec<AffixRequirement>) {
    requirements.sort_by(|left, right| {
        if left.affix_id == right.affix_id
            && left.item_type == right.item_type
            && left.is_implicit == right.is_implicit
            && left.is_tempered == right.is_tempered
        {
            return Ordering::Equal;
        }
        match (left.is_tempered, right.is_tempered) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => match (left.is_implicit, right.is_implicit) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            },
        }
    });
    requirements.dedup_by(|left, right| {
        left.affix_id == right.affix_id
            && left.item_type == right.item_type
            && left.is_implicit == right.is_implicit
            && left.is_tempered == right.is_tempered
    });
}

#[derive(Debug, Clone)]
struct ImportLookupTables {
    affix: ImportLookupIndex,
    aspect: ImportLookupIndex,
    rune: ImportLookupIndex,
    unique: ImportLookupIndex,
}

impl ImportLookupTables {
    fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Ok(Self {
            affix: load_lookup_index(
                repo,
                language,
                "Affixes",
                "DescriptionClean",
                ImportLookupScorer::Ratio,
                D4BUILDS_AFFIX_THRESHOLD,
                false,
            )?,
            aspect: load_lookup_index(
                repo,
                language,
                "Aspects",
                "Name",
                ImportLookupScorer::TokenSet,
                D4BUILDS_NAME_THRESHOLD,
                false,
            )?,
            rune: load_lookup_index(
                repo,
                language,
                "Runes",
                "Name",
                ImportLookupScorer::TokenSet,
                D4BUILDS_NAME_THRESHOLD,
                false,
            )?,
            unique: load_lookup_index(
                repo,
                language,
                "Uniques",
                "Name",
                ImportLookupScorer::TokenSet,
                D4BUILDS_NAME_THRESHOLD,
                true,
            )?,
        })
    }
}

/// Fuzzy label-keyed provider lookup (D4Builds / Mobalytics affix texts).
///
/// The fuzzy winner is a catalog row; the stored id is that row's complete
/// `IdName`, matching upstream `BuildsManagerD4Builds.ConvertItemAffix`
/// (`_affixMapDescriptionToId` maps the description to `affix.IdName`) and
/// `BuildsManagerMaxroll.ConvertItemAffix` (`_affixMapAffixTextToId`).
fn load_lookup_index(
    repo: &AssetRepository,
    language: &str,
    category: &str,
    label_field: &str,
    scorer: ImportLookupScorer,
    threshold: f64,
    dedup_by_label: bool,
) -> Result<ImportLookupIndex, String> {
    let catalog = IdentityCatalog::load(repo, language, category, label_field)?;
    let mut first_label_id = HashMap::new();
    let mut candidates = Vec::new();
    for row in catalog.rows() {
        let Some(label) = row.label.clone() else {
            continue;
        };
        if label.trim().is_empty() {
            continue;
        }
        let mut id = row.id_name.clone();
        if dedup_by_label {
            id = first_label_id
                .entry(label.clone())
                .or_insert_with(|| id.clone())
                .clone();
        }
        candidates.push(ImportLookupCandidate {
            id,
            normalized_label: normalize_import_text(&label),
            label,
        });
    }
    Ok(ImportLookupIndex {
        candidates,
        scorer,
        threshold,
    })
}

/// Mirrors upstream `AffixManager.InitTuningPrismData` +
/// `AffixManager.GetAffixTuningPrismsByIdName`: the `TuningPrisms` list
/// recorded per affix row. Upstream keys the map by the row `IdName`; the
/// Rust import lookups resolve to individual `IdNameList` entries, so the
/// row prisms are additionally reachable through every `IdNameList` entry.
/// Keys are inserted first-wins so the prism row matches the first row a
/// lookup index candidate would resolve to.
fn load_affix_tuning_prisms(
    repo: &AssetRepository,
    language: &str,
) -> Result<HashMap<String, Vec<String>>, String> {
    let path = repo.data_dir().join(format!("Affixes.{language}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let mut prisms: HashMap<String, Vec<String>> = HashMap::new();
    for item in items {
        let values: Vec<String> = item
            .get("TuningPrisms")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(id_name) = string_field(&item, "IdName") {
            prisms
                .entry(id_name.clone())
                .or_insert_with(|| values.clone());
            for segment in id_name.split(';') {
                prisms
                    .entry(segment.to_string())
                    .or_insert_with(|| values.clone());
            }
        }
        if let Some(entries) = item.get("IdNameList").and_then(|value| value.as_array()) {
            for entry in entries {
                if let Some(id_name) = entry.as_str() {
                    prisms
                        .entry(id_name.to_string())
                        .or_insert_with(|| values.clone());
                }
            }
        }
    }
    Ok(prisms)
}

fn affix_tuning_prisms(prisms: &HashMap<String, Vec<String>>, affix_id: &str) -> Vec<String> {
    prisms.get(affix_id).cloned().unwrap_or_default()
}

fn count_localized_rows(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
) -> Result<usize, ImportLocalizedDataError> {
    let path = ensure_localized_data_file(repo, language, component)?;
    let text = fs::read_to_string(&path).map_err(|error| ImportLocalizedDataError {
        language: language.to_string(),
        component,
        path: path.display().to_string(),
        message: format!("failed to read {}: {error}", path.display()),
    })?;
    let items: Vec<Value> =
        serde_json::from_str(&text).map_err(|error| ImportLocalizedDataError {
            language: language.to_string(),
            component,
            path: path.display().to_string(),
            message: format!("failed to parse {}: {error}", path.display()),
        })?;
    Ok(items.len())
}

fn localized_data_path(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
) -> PathBuf {
    repo.data_dir()
        .join(format!("{}.{}.json", component.file_stem(), language))
}

pub(crate) fn ensure_localized_data_file(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
) -> Result<PathBuf, ImportLocalizedDataError> {
    let path = localized_data_path(repo, language, component);
    if path.is_file() {
        Ok(path)
    } else {
        Err(ImportLocalizedDataError {
            language: language.to_string(),
            component,
            path: path.display().to_string(),
            message: "missing localized data file".to_string(),
        })
    }
}

fn typed_localized_data_error(
    repo: &AssetRepository,
    language: &str,
    component: LocalizedDataComponent,
    message: String,
) -> ImportLocalizedDataError {
    ImportLocalizedDataError {
        language: language.to_string(),
        component,
        path: localized_data_path(repo, language, component)
            .display()
            .to_string(),
        message,
    }
}

fn string_field(item: &Value, field: &str) -> Option<String> {
    item.get(field).and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn normalize_import_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            for lowered in ch.to_lowercase() {
                out.push(lowered);
            }
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn replace_ascii_case_insensitive(value: &str, pattern: &str, replacement: &str) -> String {
    if pattern.is_empty() {
        return value.to_string();
    }

    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some((start, _)) = value[cursor..].char_indices().find(|(offset, _)| {
        value[cursor + offset..]
            .get(..pattern.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(pattern))
    }) {
        let start = cursor + start;
        if value[start..]
            .get(..pattern.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(pattern))
        {
            output.push_str(&value[cursor..start]);
            output.push_str(replacement);
            cursor = start + pattern.len();
        } else {
            let next = start + value[start..].chars().next().map_or(1, char::len_utf8);
            output.push_str(&value[cursor..next]);
            cursor = next;
        }
    }

    output.push_str(&value[cursor..]);
    output
}

fn score_import_text(scorer: ImportLookupScorer, left: &str, right: &str) -> f64 {
    match scorer {
        ImportLookupScorer::Ratio => ratio_import_score(left, right),
        ImportLookupScorer::TokenSet => token_set_import_score(left, right),
    }
}

fn ratio_import_score(left: &str, right: &str) -> f64 {
    if left == right {
        return 100.0;
    }
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 100.0;
    }
    let distance = levenshtein_import(left, right);
    ((max_len.saturating_sub(distance)) as f64 / max_len as f64) * 100.0
}

fn token_set_import_score(left: &str, right: &str) -> f64 {
    if left == right {
        return 100.0;
    }
    let left_tokens: Vec<&str> = left.split_whitespace().collect();
    let right_tokens: Vec<&str> = right.split_whitespace().collect();
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return ratio_import_score(left, right);
    }

    let mut shared = Vec::new();
    let mut left_only = Vec::new();
    let mut right_only = Vec::new();

    for token in &left_tokens {
        if right_tokens.contains(token) {
            shared.push(*token);
        } else {
            left_only.push(*token);
        }
    }
    for token in &right_tokens {
        if !left_tokens.contains(token) {
            right_only.push(*token);
        }
    }

    let shared_joined = shared.join(" ");
    let left_joined = if left_only.is_empty() {
        shared_joined.clone()
    } else {
        format!("{shared_joined} {}", left_only.join(" "))
            .trim()
            .to_string()
    };
    let right_joined = if right_only.is_empty() {
        shared_joined.clone()
    } else {
        format!("{shared_joined} {}", right_only.join(" "))
            .trim()
            .to_string()
    };

    ratio_import_score(&shared_joined, &left_joined)
        .max(ratio_import_score(&shared_joined, &right_joined))
        .max(ratio_import_score(&left_joined, &right_joined))
}

fn levenshtein_import(left: &str, right: &str) -> usize {
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.is_empty() {
        return right_chars.len();
    }
    if right_chars.is_empty() {
        return left_chars.len();
    }

    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left_chars.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            current[right_index + 1] = (current[right_index] + 1)
                .min(previous[right_index + 1] + 1)
                .min(previous[right_index] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_chars.len()]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
