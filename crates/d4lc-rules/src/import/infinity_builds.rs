use crate::import::{
    affix_tuning_prisms, ensure_cache_filename_safe, flatten_paragon_routes,
    load_affix_tuning_prisms, load_exact_id_lookup_index, paragon_rotation_text,
    push_import_warning, score_import_text, sort_and_dedup_affix_requirements, sorted_json_files,
    string_field, ImportBuildDescriptor, ImportColors, ImportExactIdLookupIndex,
    ImportLookupScorer, ImportUnmappedEntry, ImportVariantDescriptor, ImportWarning,
    ImportWorkflowResult, ImportedItemAffixes, ImportedParagonBoard, ImportedParagonRoute,
    EQUIPMENT_ITEM_TYPES,
};
use crate::model::{
    AffixPreset, AffixRequirement, NonAffixPreset, NonAffixRequirement, RuleNonAffixKind,
};
use d4lc_assets::AssetRepository;
use d4lc_config::AppSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const INFINITY_BUILDS_PROVIDER: &str = "InfinityBuilds";

const INFINITY_BUILDS_LOAD_AVAILABLE_METHOD: &str =
    "BuildsManagerInfinityBuilds.LoadAvailableInfinityBuildsBuilds";

const INFINITY_BUILDS_DOWNLOAD_METHOD: &str =
    "BuildsManagerInfinityBuilds.DownloadInfinityBuildsBuild";

const INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD: &str = "BuildsManagerInfinityBuilds.ParseJsonBuild";

const INFINITY_BUILDS_EXPORT_VARIANT_METHOD: &str =
    "BuildsManagerInfinityBuilds.ExportBuildVariant";

const INFINITY_BUILDS_CONVERT_AFFIX_METHOD: &str = "BuildsManagerInfinityBuilds.ConvertItemAffix";

const INFINITY_BUILDS_CONVERT_ASPECT_METHOD: &str = "BuildsManagerInfinityBuilds.ConvertItemAspect";

const INFINITY_BUILDS_PARAGON_METHOD: &str = "BuildsManagerInfinityBuilds.GetAllParagonBoards";

const INFINITY_BUILDS_CREATE_PRESET_METHOD: &str =
    "BuildsManagerInfinityBuilds.CreatePresetFromInfinityBuildsBuild";

const INFINITY_BUILDS_LANGUAGE: &str = "enUS";

// ---------------------------------------------------------------------------
// InfinityBuilds.gg provider.
//
// Mirrors upstream `D4Companion.Services.BuildsManagerInfinityBuilds`:
// the site embeds the build payload in a Next.js flight script; upstream
// captures it through a Selenium/DevTools browser session. The Rust port
// is not browser-driven — the equivalent mechanism is a direct HTTP fetch
// of the same page followed by the identical script extraction
// (`extract_infinity_builds_share_script` + `parse_infinity_builds_share_html`,
// parity `equivalent` for the fetch, `exact` for the JSON navigation).
//
// Pipeline (upstream method per step):
//   1. `DownloadInfinityBuildsBuild`  — app-side fetch (ureq seam), HTML in.
//   2. `ParseJsonBuild`               — `parse_infinity_builds_share_html`.
//   3. `ExportBuildVariants` / `ExportBuildVariant` — raw per-variant export.
//   4. `ConvertBuildVariants` / `ConvertItem*`      — data lookup conversion.
//   5. Build cached to `Builds/InfinityBuilds/{id}.json` —
//      `export_infinity_builds_build_cache` + `LoadAvailableInfinityBuildsBuilds`
//      (`load_available_infinity_builds_variants`).
//   6. `CreatePresetFromInfinityBuildsBuild` —
//      `import_infinity_builds_build_variant` reads the cached conversion.
//
// Upstream caveat preserved: the aspect/unique NAME maps are
// commented "currently not useable" upstream because InfinityBuilds mixes
// English and Chinese names; conversion therefore matches by IDs
// (affix/aspect: `IdNameList` membership, rune: `IdName` equality,
// unique: best fuzzy match over `IdName`, mirroring upstream's
// FuzzierSharp `Process.ExtractOne` with the repo's ratio scorer).
// ---------------------------------------------------------------------------

/// Cached InfinityBuilds build. Shape mirrors upstream
/// `D4Companion.Entities.InfinityBuildsBuild` (PascalCase JSON keys so a
/// cache file round-trips like upstream's `Builds/InfinityBuilds/*.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfinityBuildsBuild {
    #[serde(rename = "Date", default)]
    pub date: String,
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Url", default)]
    pub url: String,
    #[serde(rename = "Variants", default)]
    pub variants: Vec<InfinityBuildsBuildVariant>,
}

/// Cached InfinityBuilds build variant (upstream `InfinityBuildsBuildVariant`).
/// The converted affix preset plus the converted non-affix entries live here:
/// upstream stores `ItemAspects` / `ItemRunes` / `ItemUniques` on
/// `AffixPreset`; the Rust `AffixPreset` type only carries affixes, so the
/// converted non-affix requirements sit beside it on the variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfinityBuildsBuildVariant {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(
        rename = "AffixPreset",
        default = "infinity_builds_default_affix_preset"
    )]
    pub affix_preset: AffixPreset,
    #[serde(rename = "Helm", default)]
    pub helm: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Chest", default)]
    pub chest: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Gloves", default)]
    pub gloves: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Pants", default)]
    pub pants: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Boots", default)]
    pub boots: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Amulet", default)]
    pub amulet: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Ring", default)]
    pub ring: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Weapon", default)]
    pub weapon: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Ranged", default)]
    pub ranged: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Offhand", default)]
    pub offhand: Vec<InfinityBuildsAffix>,
    #[serde(rename = "Aspect", default)]
    pub aspect: Vec<String>,
    #[serde(rename = "Uniques", default)]
    pub uniques: Vec<String>,
    #[serde(rename = "Runes", default)]
    pub runes: Vec<String>,
    #[serde(rename = "ParagonBoards", default)]
    pub paragon_boards: Vec<ImportedParagonBoard>,
    #[serde(rename = "ItemAspects", default)]
    pub item_aspects: Vec<NonAffixRequirement>,
    #[serde(rename = "ItemUniques", default)]
    pub item_uniques: Vec<NonAffixRequirement>,
    #[serde(rename = "ItemRunes", default)]
    pub item_runes: Vec<NonAffixRequirement>,
}

/// Raw InfinityBuilds affix entry (upstream `InfinityBuildsAffix`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfinityBuildsAffix {
    #[serde(rename = "AffixText", default)]
    pub affix_text: String,
    #[serde(rename = "AffixTextList", default)]
    pub affix_text_list: Vec<String>,
    #[serde(rename = "IsGreater", default)]
    pub is_greater: bool,
    #[serde(rename = "IsImplicit", default)]
    pub is_implicit: bool,
    #[serde(rename = "IsTempered", default)]
    pub is_tempered: bool,
}

fn infinity_builds_default_affix_preset() -> AffixPreset {
    AffixPreset {
        name: String::new(),
        affixes: Vec::new(),
    }
}

/// Result of the share-page import pipeline: the converted build (ready for
/// the local cache) plus the structured warnings raised during conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfinityBuildsShareImport {
    pub build: InfinityBuildsBuild,
    pub warnings: Vec<ImportWarning>,
    pub unmapped_entries: Vec<ImportUnmappedEntry>,
}

// -- Embedded site JSON entities (upstream `InfinityBuildsBuildJson.cs`). --

#[derive(Debug, Clone, Deserialize)]
struct InfinityBuildsContainerJson {
    #[serde(default, rename = "id")]
    id: String,
    #[serde(default, rename = "children")]
    children: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct InfinityBuildsWrapperJson {
    #[serde(default, rename = "build")]
    build: InfinityBuildsBuildJson,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct InfinityBuildsBuildJson {
    #[serde(default, rename = "title")]
    title: String,
    #[serde(default, rename = "shareSlug")]
    share_slug: String,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
    #[serde(default, rename = "variants")]
    variants: Vec<InfinityBuildsBuildVariantJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct InfinityBuildsBuildVariantJson {
    #[serde(default, rename = "name")]
    name: String,
    #[serde(default, rename = "gear")]
    gear: Vec<InfinityBuildsBuildVariantGearJson>,
    #[serde(default, rename = "paragon")]
    paragon: InfinityBuildsBuildParagonJson,
}

#[derive(Debug, Clone, Deserialize)]
struct InfinityBuildsBuildVariantGearJson {
    /// custom_legendary, mythic, unique (upstream doc comment).
    #[serde(default, rename = "kind")]
    kind: String,
    #[serde(default, rename = "slot")]
    slot: String,
    #[serde(default, rename = "itemId")]
    item_id: String,
    #[serde(default, rename = "affixes")]
    affixes: Vec<InfinityBuildsBuildVariantAffixJson>,
    #[serde(default, rename = "sockets")]
    sockets: Vec<String>,
    #[serde(default, rename = "aspectId")]
    aspect_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct InfinityBuildsBuildVariantAffixJson {
    #[serde(default, rename = "affixId")]
    affix_id: String,
    #[serde(default, rename = "greater")]
    greater: bool,
    #[serde(default, rename = "tempered")]
    tempered: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct InfinityBuildsBuildParagonJson {
    #[serde(default, rename = "slots")]
    slots: Vec<InfinityBuildsBuildParagonSlotJson>,
    #[serde(default, rename = "glyphs")]
    glyphs: BTreeMap<String, String>,
    #[serde(default, rename = "activeNodes")]
    active_nodes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct InfinityBuildsBuildParagonSlotJson {
    #[serde(default, rename = "boardId")]
    board_id: String,
    #[serde(default, rename = "rotation")]
    rotation: i64,
}

// -- Lookup data. -----------------------------------------------------------

/// One localized data row keyed by `IdNameList` membership. Mirrors upstream
/// `_affixes.FirstOrDefault(a => a.IdNameList.Contains(id, OrdinalIgnoreCase))`.
#[derive(Debug, Clone)]
struct InfinityBuildsIdNameRow {
    id_name: String,
    id_names: Vec<String>,
    label: Option<String>,
}

impl InfinityBuildsIdNameRow {
    fn contains_id(&self, id: &str) -> bool {
        self.id_names
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(id))
    }
}

#[derive(Debug, Clone)]
struct InfinityBuildsLookupTables {
    affixes: Vec<InfinityBuildsIdNameRow>,
    aspects: Vec<InfinityBuildsIdNameRow>,
    runes: Vec<String>,
    uniques: Vec<String>,
    paragon_board: ImportExactIdLookupIndex,
    paragon_glyph: ImportExactIdLookupIndex,
    tuning_prisms_by_id: HashMap<String, Vec<String>>,
}

impl InfinityBuildsLookupTables {
    fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Ok(Self {
            affixes: load_infinity_builds_id_name_rows(repo, language, "Affixes")?,
            aspects: load_infinity_builds_id_name_rows(repo, language, "Aspects")?,
            runes: load_infinity_builds_id_name_values(repo, language, "Runes")?,
            uniques: load_infinity_builds_id_name_values(repo, language, "Uniques")?,
            paragon_board: load_exact_id_lookup_index(repo, language, "ParagonBoards")?,
            paragon_glyph: load_exact_id_lookup_index(repo, language, "ParagonGlyphs")?,
            tuning_prisms_by_id: load_affix_tuning_prisms(repo, language)?,
        })
    }

    fn match_affix_or_aspect(&self, id: &str, aspects: bool) -> Option<&InfinityBuildsIdNameRow> {
        let rows = if aspects {
            &self.aspects
        } else {
            &self.affixes
        };
        rows.iter().find(|row| row.contains_id(id))
    }

    fn match_rune(&self, id: &str) -> Option<String> {
        self.runes
            .iter()
            .find(|entry| entry.eq_ignore_ascii_case(id))
            .cloned()
    }

    fn match_unique(&self, id: &str) -> String {
        // Mirrors upstream `Process.ExtractOne(unique, _uniqueIds,
        // WeightedRatioScorer)`: best-scoring `IdName`, first candidate wins
        // ties, no threshold (ExtractOne always returns a match). Scoring is
        // the repo's ratio scorer applied case-insensitively; upstream's
        // FuzzierSharp WeightedRatio is case-sensitive, but identifiers only
        // differ in case so the best match is the same.
        let id_lower = id.to_ascii_lowercase();
        let mut best: Option<(f64, &String)> = None;
        for candidate in &self.uniques {
            let score = score_import_text(
                ImportLookupScorer::Ratio,
                &candidate.to_ascii_lowercase(),
                &id_lower,
            );
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, candidate));
            }
        }
        best.map(|(_, value)| value.clone()).unwrap_or_default()
    }
}

fn load_infinity_builds_id_name_rows(
    repo: &AssetRepository,
    language: &str,
    category: &str,
) -> Result<Vec<InfinityBuildsIdNameRow>, String> {
    let path = repo.data_dir().join(format!("{category}.{language}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let mut rows = Vec::new();
    for item in items {
        let Some(id_name) = string_field(&item, "IdName") else {
            continue;
        };
        let id_names = match item.get("IdNameList").and_then(|value| value.as_array()) {
            Some(values) => values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect(),
            None => vec![id_name.clone()],
        };
        let label = string_field(&item, "DescriptionClean");
        rows.push(InfinityBuildsIdNameRow {
            id_name,
            id_names,
            label,
        });
    }
    Ok(rows)
}

fn load_infinity_builds_id_name_values(
    repo: &AssetRepository,
    language: &str,
    category: &str,
) -> Result<Vec<String>, String> {
    let path = repo.data_dir().join(format!("{category}.{language}.json"));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let items: Vec<Value> = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    Ok(items
        .iter()
        .filter_map(|item| string_field(item, "IdName"))
        .collect())
}

// -- Script extraction + embedded JSON navigation. --------------------------

/// Return the first `<script>` block whose content contains `shareSlug`.
/// Mirrors the upstream browser-side scan
/// `scripts.find(s => s.textContent.includes('shareSlug'))`.
pub fn extract_infinity_builds_share_script(html: &str) -> Option<String> {
    extract_infinity_builds_share_scripts(html)
        .into_iter()
        .next()
}

/// Return every `<script>` block whose content contains `shareSlug`, in
/// document order. Upstream scans only the first match; on today's share
/// pages an earlier script (chunk manifest) can mention `shareSlug`
/// without carrying the build payload, so the navigation must be able to
/// try the later candidates.
pub fn extract_infinity_builds_share_scripts(html: &str) -> Vec<String> {
    let mut scripts = Vec::new();
    let mut cursor = 0;
    while let Some(tag_start) = html[cursor..].find("<script") {
        let absolute_tag_start = cursor + tag_start;
        let content_start = match html[absolute_tag_start..].find('>') {
            Some(offset) => absolute_tag_start + offset + 1,
            None => break,
        };
        let content_end = match html[content_start..].find("</script>") {
            Some(offset) => offset + content_start,
            None => break,
        };
        let content = &html[content_start..content_end];
        if content.contains("shareSlug") {
            scripts.push(content.to_string());
        }
        cursor = content_end + "</script>".len();
    }
    scripts
}

/// C# `JsonElement.ToString()` semantics used by the upstream navigation:
/// strings unwrap their quotes, everything else serializes as JSON text.
fn json_element_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Parse the share page and navigate the Next.js flight payload to the
/// embedded `InfinityBuildsBuildJson`. Direct port of upstream
/// `BuildsManagerInfinityBuilds.DownloadInfinityBuildsBuild` (script surgery)
/// plus `ParseJsonBuild` (index navigation with the nested-children fallback).
/// Every `shareSlug` script is tried in document order and the first one
/// that navigates to a build wins.
fn parse_infinity_builds_share_html(html: &str) -> Result<InfinityBuildsBuildJson, String> {
    let scripts = extract_infinity_builds_share_scripts(html);
    if scripts.is_empty() {
        return Err(format!(
            "{INFINITY_BUILDS_DOWNLOAD_METHOD}: share page contains no script with shareSlug"
        ));
    }
    let mut last_error = String::new();
    for script_content in &scripts {
        match parse_infinity_builds_share_script(script_content) {
            Ok(build) => return Ok(build),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn parse_infinity_builds_share_script(
    script_content: &str,
) -> Result<InfinityBuildsBuildJson, String> {
    // Remove self.__next_f.push: everything after the first "(" up to the
    // trailing ")" (upstream Substring(indexOf("(") + 1) + Remove(Length-1)).
    let unwrapped = script_content
        .split_once('(')
        .map(|(_, rest)| rest)
        .unwrap_or(script_content);
    let unwrapped = unwrapped.strip_suffix(')').unwrap_or(unwrapped);

    let json: Vec<Value> = serde_json::from_str(unwrapped).map_err(|error| {
        format!("{INFINITY_BUILDS_DOWNLOAD_METHOD}: failed to parse share push payload: {error}")
    })?;
    let json_as_string = json.get(1).map(json_element_to_string).ok_or_else(|| {
        format!("{INFINITY_BUILDS_DOWNLOAD_METHOD}: share push payload has no element at index 1")
    })?;
    let build_json_start = json_as_string.find('[').ok_or_else(|| {
        format!("{INFINITY_BUILDS_DOWNLOAD_METHOD}: share push payload has no build array")
    })?;
    parse_infinity_builds_json(&json_as_string[build_json_start..])
}

/// Return every top-level balanced JSON array in `text` in document order.
/// The live Next.js flight payload packs several stream segments into one
/// push string (module references like `5e:I[...]` first, data segments
/// after), so the build array is not necessarily the first one
/// (upstream's `Substring(indexOf('['))` + deserialize relies on it being
/// the only one). Nested arrays are consumed with their parent, string
/// literals and escapes are respected.
fn top_level_balanced_json_arrays(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut arrays = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(offset) = bytes[cursor..].iter().position(|byte| *byte == b'[') else {
            break;
        };
        let start = cursor + offset;
        let Some(length) = balanced_json_array_length(bytes, start) else {
            break;
        };
        arrays.push(&text[start..start + length]);
        cursor = start + length;
    }
    arrays
}

/// Length of the balanced array starting at `start` (`[`), or `None`.
fn balanced_json_array_length(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_infinity_builds_json(json: &str) -> Result<InfinityBuildsBuildJson, String> {
    let candidates = top_level_balanced_json_arrays(json);
    if candidates.is_empty() {
        return Err(format!(
            "{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: share build array is not balanced"
        ));
    }
    let mut last_error = String::new();
    for candidate in candidates {
        match parse_infinity_builds_json_candidate(candidate) {
            Ok(build) => return Ok(build),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn parse_infinity_builds_json_candidate(json: &str) -> Result<InfinityBuildsBuildJson, String> {
    let root: Vec<Value> = serde_json::from_str(json).map_err(|error| {
        format!(
            "{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse share build array: {error}"
        )
    })?;
    // A candidate can be the wrapper array itself (`[…, {"build": {…}}]`)
    // rather than a container with `children`: live pages such as
    // `blazing-scream-warlock-VVnCwkWiey` carry the build directly at index 3.
    // The container navigation below would find no children and fail on the
    // empty nested payload, dropping the whole page.
    if let Some(build) = root.get(3).and_then(|value| value.get("build")) {
        // An empty `{"build": {}}` candidate is skipped so that it cannot
        // shadow a later real candidate.
        let named = ["shareSlug", "title"].iter().any(|key| {
            build
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        });
        let has_variants = build
            .get("variants")
            .and_then(Value::as_array)
            .is_some_and(|variants| !variants.is_empty());
        if named || has_variants {
            return infinity_builds_wrapper_from_array(&root);
        }
    }
    // Assume index 3 always contains the build data.
    let container: InfinityBuildsContainerJson = serde_json::from_value(
        root.get(3).cloned().ok_or_else(|| {
            format!("{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: share build array has no element at index 3")
        })?,
    )
    .map_err(|error| {
        format!("{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse share container: {error}")
    })?;

    // Index of property Children containing build data varies. Also there can
    // be an extra nested property of Children.
    let mut json_as_string = String::new();
    for child in &container.children {
        json_as_string = child.to_string();
        if json_as_string.contains("shareSlug") {
            break;
        }
    }

    // Test for nested Children property - nested if Id is not set.
    if container.id.trim().is_empty() {
        let nested_root: Vec<Value> = serde_json::from_str(&json_as_string).map_err(|error| {
            format!("{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse nested share array: {error}")
        })?;
        let nested_container: InfinityBuildsContainerJson = serde_json::from_value(
            nested_root.get(3).cloned().ok_or_else(|| {
                format!(
                    "{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: nested share array has no element at index 3"
                )
            })?,
        )
        .map_err(|error| {
            format!("{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse nested share container: {error}")
        })?;
        json_as_string = String::new();
        for child in &nested_container.children {
            json_as_string = child.to_string();
            if json_as_string.contains("shareSlug") {
                break;
            }
        }
    }

    let build_wrapper: Vec<Value> = serde_json::from_str(&json_as_string).map_err(|error| {
        format!("{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse share build wrapper: {error}")
    })?;
    infinity_builds_wrapper_from_array(&build_wrapper)
}

/// The embedded build in a wrapper array `[…, {"build": {…}}]`. Live share
/// pages deliver this array either as the top-level candidate itself or
/// nested under a container's children.
fn infinity_builds_wrapper_from_array(array: &[Value]) -> Result<InfinityBuildsBuildJson, String> {
    // Assume index 3 always contains the build data.
    let wrapper: InfinityBuildsWrapperJson = serde_json::from_value(
        array.get(3).cloned().ok_or_else(|| {
            format!(
                "{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: share build wrapper has no element at index 3"
            )
        })?,
    )
    .map_err(|error| {
        format!(
            "{INFINITY_BUILDS_PARSE_JSON_BUILD_METHOD}: failed to parse share build wrapper object: {error}"
        )
    })?;
    Ok(wrapper.build)
}

// -- Export (raw variant extraction). ---------------------------------------

fn infinity_builds_affixes_for_slot(
    build_variant: &InfinityBuildsBuildVariantJson,
    item_type: &str,
) -> Vec<InfinityBuildsAffix> {
    let mut affixes = Vec::new();
    for gear_entry in &build_variant.gear {
        if !gear_entry.slot.eq_ignore_ascii_case(item_type) {
            continue;
        }
        for affix in &gear_entry.affixes {
            affixes.push(InfinityBuildsAffix {
                affix_text: affix.affix_id.clone(),
                affix_text_list: affix.affix_id.split('-').map(ToOwned::to_owned).collect(),
                is_greater: affix.greater,
                is_implicit: false, // Not available (upstream comment).
                is_tempered: affix.tempered,
            });
        }
    }
    affixes
}

fn infinity_builds_aspects(build_variant: &InfinityBuildsBuildVariantJson) -> Vec<String> {
    build_variant
        .gear
        .iter()
        .filter(|gear_entry| !gear_entry.aspect_id.trim().is_empty())
        .map(|gear_entry| gear_entry.aspect_id.clone())
        .collect()
}

fn infinity_builds_runes(build_variant: &InfinityBuildsBuildVariantJson) -> Vec<String> {
    build_variant
        .gear
        .iter()
        .flat_map(|gear_entry| gear_entry.sockets.iter())
        .filter(|socket| socket.to_ascii_lowercase().contains("-rune-"))
        .cloned()
        .collect()
}

fn infinity_builds_uniques(build_variant: &InfinityBuildsBuildVariantJson) -> Vec<String> {
    // Upstream `GetAllUniques` compares Kind with default (case-sensitive)
    // equality while `GetAllAffixes` uses OrdinalIgnoreCase; preserved as-is.
    build_variant
        .gear
        .iter()
        .filter(|gear_entry| gear_entry.kind == "unique" || gear_entry.kind == "mythic")
        .map(|gear_entry| gear_entry.item_id.clone())
        .collect()
}

/// Paragon board name/glyph split helpers (`prefix::Name[::location]`).
fn infinity_builds_split_segment(value: &str) -> Result<String, String> {
    value
        .split("::")
        .nth(1)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("segment missing in '{value}' (expected ::-separated id)"))
}

fn infinity_builds_rotation_index(rotation: &str) -> usize {
    match rotation {
        "0°" => 0,
        "90°" => 1,
        "180°" => 2,
        "270°" => 3,
        _ => 0,
    }
}

/// Direct port of upstream `GetAllParagonBoards` including the rotation
/// transform for active node locations on a 21x21 grid.
fn infinity_builds_paragon_boards(
    build_variant: &InfinityBuildsBuildVariantJson,
    lookups: &InfinityBuildsLookupTables,
) -> Result<Vec<ImportedParagonBoard>, String> {
    let paragon = &build_variant.paragon;
    if paragon.slots.is_empty() || paragon.active_nodes.is_empty() {
        return Ok(Vec::new());
    }

    struct RawBoard {
        name: String,
        rotation: String,
        glyph: String,
        nodes: Vec<usize>,
    }

    let mut boards: Vec<RawBoard> = Vec::new();

    // Starter board.
    boards.push(RawBoard {
        name: infinity_builds_split_segment(
            paragon
                .active_nodes
                .first()
                .map(String::as_str)
                .unwrap_or(""),
        )?,
        rotation: "0°".to_string(),
        glyph: String::new(),
        nodes: Vec::new(),
    });

    // Other boards.
    for board in &paragon.slots {
        boards.push(RawBoard {
            name: infinity_builds_split_segment(&board.board_id)?,
            rotation: paragon_rotation_text(board.rotation).to_string(),
            glyph: String::new(),
            nodes: Vec::new(),
        });
    }

    // Update boards: glyph + active node locations.
    for board in &mut boards {
        // Glyph keys embed the raw board name between "::" markers.
        let marker = format!("::{}::", board.name);
        if let Some((_, glyph)) = paragon.glyphs.iter().find(|(key, _)| {
            key.to_ascii_lowercase()
                .contains(&marker.to_ascii_lowercase())
        }) {
            board.glyph = glyph.get(7..).unwrap_or("").to_string(); // Remove "glyph::" prefix.
        }

        for node in &paragon.active_nodes {
            let node_data: Vec<&str> = node.split("::").collect();
            let node_board = node_data
                .get(1)
                .ok_or_else(|| format!("paragon node '{node}' has no board segment"))?;
            let node_location: usize = node_data
                .get(2)
                .ok_or_else(|| format!("paragon node '{node}' has no location segment"))?
                .parse()
                .map_err(|error| {
                    format!("paragon node '{node}' has an invalid location: {error}")
                })?;
            if node_board != &board.name {
                continue;
            }

            let rotation = infinity_builds_rotation_index(&board.rotation);
            let location_x = node_location % 21;
            let location_y = node_location / 21;
            let location = match rotation {
                1 => {
                    let x_t = 21 - location_y - 1;
                    let y_t = location_x;
                    y_t * 21 + x_t
                }
                2 => {
                    let x_t = 21 - location_x - 1;
                    let y_t = 21 - location_y - 1;
                    y_t * 21 + x_t
                }
                3 => {
                    let x_t = location_y;
                    let y_t = 21 - location_x - 1;
                    y_t * 21 + x_t
                }
                _ => node_location,
            };
            if !board.nodes.contains(&location) {
                board.nodes.push(location);
            }
        }
        board.nodes.sort_unstable();
    }

    // Fix naming inconsistencies.
    let mut result = Vec::new();
    for board in boards {
        let name = board.name.replace('-', "_");
        let glyph = board.glyph.replace('-', "_");
        result.push(ImportedParagonBoard {
            index: 0,
            name: lookups
                .paragon_board
                .match_id_case_insensitive(&name)
                .unwrap_or_else(|| name.clone()),
            glyph: lookups
                .paragon_glyph
                .match_id_case_insensitive(&glyph)
                .unwrap_or_else(|| glyph.clone()),
            rotation: board.rotation,
            active_nodes: board.nodes,
        });
    }
    Ok(result)
}

/// Upstream `ExportBuildVariants` + `ExportBuildVariant`: project the share
/// JSON variant into the cached raw-variant shape. Paragon boards follow the
/// `IsImportParagonInfinityBuildsEnabled` gate exactly like upstream.
fn infinity_builds_export_variant(
    build_variant: &InfinityBuildsBuildVariantJson,
) -> InfinityBuildsBuildVariant {
    let mut variant = InfinityBuildsBuildVariant {
        name: build_variant.name.clone(),
        affix_preset: AffixPreset {
            name: build_variant.name.clone(),
            affixes: Vec::new(),
        },
        helm: Vec::new(),
        chest: Vec::new(),
        gloves: Vec::new(),
        pants: Vec::new(),
        boots: Vec::new(),
        amulet: Vec::new(),
        ring: Vec::new(),
        weapon: Vec::new(),
        ranged: Vec::new(),
        offhand: Vec::new(),
        aspect: Vec::new(),
        uniques: Vec::new(),
        runes: Vec::new(),
        paragon_boards: Vec::new(),
        item_aspects: Vec::new(),
        item_uniques: Vec::new(),
        item_runes: Vec::new(),
    };

    variant.aspect = infinity_builds_aspects(build_variant);
    variant.uniques = infinity_builds_uniques(build_variant);

    // Armor
    variant.helm = infinity_builds_affixes_for_slot(build_variant, "helm");
    variant.chest = infinity_builds_affixes_for_slot(build_variant, "chest");
    variant.gloves = infinity_builds_affixes_for_slot(build_variant, "gloves");
    variant.pants = infinity_builds_affixes_for_slot(build_variant, "pants");
    variant.boots = infinity_builds_affixes_for_slot(build_variant, "boots");

    // Accessories. Upstream applies `Distinct()` to Ring / Weapon; on the
    // C# reference-typed `InfinityBuildsAffix` that dedupe is a no-op, so
    // the entries are kept verbatim here.
    variant.amulet = infinity_builds_affixes_for_slot(build_variant, "amulet");
    let ring_affixes = infinity_builds_affixes_for_slot(build_variant, "ring1");
    variant.ring.extend(ring_affixes);
    let ring_affixes = infinity_builds_affixes_for_slot(build_variant, "ring2");
    variant.ring.extend(ring_affixes);

    // Weapons. Ranged stays empty: InfinityBuilds does not separate melee
    // and ranged weapons (upstream comment).
    let weapon_affixes = infinity_builds_affixes_for_slot(build_variant, "mainhand");
    variant.weapon.extend(weapon_affixes);
    let weapon_affixes = infinity_builds_affixes_for_slot(build_variant, "offhandWeapon");
    variant.weapon.extend(weapon_affixes);
    let weapon_affixes = infinity_builds_affixes_for_slot(build_variant, "twoHander");
    variant.weapon.extend(weapon_affixes);
    let weapon_affixes = infinity_builds_affixes_for_slot(build_variant, "weapon");
    variant.weapon.extend(weapon_affixes);
    variant.offhand = infinity_builds_affixes_for_slot(build_variant, "offhand");

    // Runes
    variant.runes = infinity_builds_runes(build_variant);

    variant
}

// -- Conversion (data lookups + structured warnings). -----------------------

/// Upstream `ConvertItemAffix`: "affix-s04-life" -> "s04_life", matched by
/// `IdNameList` membership. Tuning prisms attach through the resolved
/// `IdName` exactly like upstream `GetAffixTuningPrismsByIdName`.
fn infinity_builds_convert_item_affix(
    lookups: &InfinityBuildsLookupTables,
    item_type: &str,
    affix: &InfinityBuildsAffix,
    colors: &ImportColors,
) -> (AffixRequirement, Option<String>) {
    let mut infinity_builds_affix_id = affix.affix_text.replace('-', "_");
    // Remove "affix_" prefix (upstream Substring(6)); malformed short ids
    // resolve to empty and surface through the unknown-affix warning.
    infinity_builds_affix_id = infinity_builds_affix_id
        .get(6..)
        .unwrap_or_default()
        .to_string();

    let matched = lookups.match_affix_or_aspect(&infinity_builds_affix_id, false);
    let affix_id_name = matched.map_or(String::new(), |row| row.id_name.clone());
    let tuning_prisms = affix_tuning_prisms(&lookups.tuning_prisms_by_id, &affix_id_name);
    let requirement = AffixRequirement {
        affix_id: affix_id_name.clone(),
        label: matched.and_then(|row| row.label.clone()),
        item_type: item_type.to_string(),
        // Upstream `BuildsManagerInfinityBuilds.cs:647-650`: implicit, then
        // greater, then tempered, else the configured normal color.
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
    let warning = (affix_id_name.is_empty()).then(|| affix.affix_text.clone());
    (requirement, warning)
}

/// Upstream `ConvertItemAspect`: "aspect-asp-legendary-paladin-031-asp" ->
/// "legendary_paladin_031", with the `_x1` / `_x2` fallbacks upstream adds
/// for old / incorrect InfinityBuilds ids.
fn infinity_builds_convert_item_aspect(
    lookups: &InfinityBuildsLookupTables,
    aspect: &str,
) -> Option<String> {
    let mut infinity_builds_aspect_id = aspect.replace('-', "_");
    if infinity_builds_aspect_id.contains("legendary") {
        let start = infinity_builds_aspect_id
            .find("legendary")
            .unwrap_or_default();
        infinity_builds_aspect_id = infinity_builds_aspect_id[start..].to_string();
    } else if infinity_builds_aspect_id.contains("_asp_") {
        let start = infinity_builds_aspect_id.find("_asp_").unwrap_or_default() + 5;
        infinity_builds_aspect_id = infinity_builds_aspect_id[start..].to_string();
    }
    // Remove "_asp" suffix (upstream Remove(Length - 4)).
    let trimmed_len = infinity_builds_aspect_id.len().saturating_sub(4);
    infinity_builds_aspect_id = infinity_builds_aspect_id[..trimmed_len].to_string();

    if let Some(matched) = lookups.match_affix_or_aspect(&infinity_builds_aspect_id, true) {
        return Some(matched.id_name.clone());
    }
    if let Some(matched) =
        lookups.match_affix_or_aspect(&format!("{infinity_builds_aspect_id}_x1"), true)
    {
        return Some(matched.id_name.clone());
    }
    if let Some(matched) =
        lookups.match_affix_or_aspect(&format!("{infinity_builds_aspect_id}_x2"), true)
    {
        return Some(matched.id_name.clone());
    }
    None
}

/// Upstream `ConvertItemRune`: "item-rune-effect-summonspiritwolf-itm" ->
/// "item_rune_effect_summonspiritwolf", matched by exact `IdName`
/// (case-insensitive). No warning upstream on a miss; empty id.
fn infinity_builds_convert_item_rune(lookups: &InfinityBuildsLookupTables, rune: &str) -> String {
    let mut infinity_builds_rune_id = rune.replace('-', "_");
    let trimmed_len = infinity_builds_rune_id.len().saturating_sub(4); // Remove "_itm" suffix.
    infinity_builds_rune_id = infinity_builds_rune_id[..trimmed_len].to_string();
    let suffix_start = infinity_builds_rune_id
        .find("_rune_")
        .map(|index| index + 6)
        .unwrap_or_default();
    infinity_builds_rune_id =
        infinity_builds_rune_id[suffix_start.min(infinity_builds_rune_id.len())..].to_string();
    let infinity_builds_rune_id = format!("item_rune_{infinity_builds_rune_id}");
    lookups
        .match_rune(&infinity_builds_rune_id)
        .unwrap_or_default()
}

/// Upstream `ConvertBuildVariants` for one variant: converts affixes,
/// aspects, runes and uniques, fills the converted preset plus the
/// converted non-affix entries, and records structured warnings for
/// unknown affixes / aspects (upstream `WarningOccurredMessage`s).
fn infinity_builds_convert_variant(
    variant: &mut InfinityBuildsBuildVariant,
    lookups: &InfinityBuildsLookupTables,
    colors: &ImportColors,
    warnings: &mut Vec<ImportWarning>,
    unmapped_entries: &mut Vec<ImportUnmappedEntry>,
) {
    let mut affix_preset = AffixPreset {
        name: variant.name.clone(),
        affixes: Vec::new(),
    };

    // Prepare affixes across item slots in the upstream order.
    const SLOT_AFFIXES: [(&str, &str); 10] = [
        ("helm", "helm"),
        ("chest", "chest"),
        ("gloves", "gloves"),
        ("pants", "pants"),
        ("boots", "boots"),
        ("amulet", "amulet"),
        ("ring", "ring"),
        ("weapon", "weapon"),
        ("ranged", "ranged"),
        ("offhand", "offhand"),
    ];
    for (slot_field, item_type) in SLOT_AFFIXES {
        let affixes = match slot_field {
            "helm" => &variant.helm,
            "chest" => &variant.chest,
            "gloves" => &variant.gloves,
            "pants" => &variant.pants,
            "boots" => &variant.boots,
            "amulet" => &variant.amulet,
            "ring" => &variant.ring,
            "weapon" => &variant.weapon,
            "ranged" => &variant.ranged,
            _ => &variant.offhand,
        };
        for (index, affix) in affixes.iter().enumerate() {
            let (requirement, warning) =
                infinity_builds_convert_item_affix(lookups, item_type, affix, colors);
            if let Some(affix_text) = warning {
                push_import_warning(
                    INFINITY_BUILDS_PROVIDER,
                    warnings,
                    unmapped_entries,
                    format!("{slot_field}[{index}].AffixText"),
                    affix_text.clone(),
                    format!("Imported InfinityBuild build contains unknown affix: {affix_text}."),
                    INFINITY_BUILDS_CONVERT_AFFIX_METHOD,
                );
                continue; // Upstream RemoveAll on empty Id.
            }
            affix_preset.affixes.push(requirement);
        }
    }

    // Sort affixes + remove duplicates (upstream sort + DistinctBy).
    sort_and_dedup_affix_requirements(&mut affix_preset.affixes);

    // Find matching aspect ids; each converted aspect spans all equipment
    // item types like upstream `AffixPreset.ItemAspects`.
    let mut item_aspects = Vec::new();
    for (index, raw_aspect) in variant.aspect.iter().enumerate() {
        match infinity_builds_convert_item_aspect(lookups, raw_aspect) {
            Some(aspect_id) => {
                for item_type in EQUIPMENT_ITEM_TYPES {
                    item_aspects.push(NonAffixRequirement {
                        id: aspect_id.clone(),
                        item_type: Some((*item_type).to_string()),
                        color: colors.aspects.clone(),
                        kind: RuleNonAffixKind::Aspect,
                    });
                }
            }
            None => push_import_warning(
                INFINITY_BUILDS_PROVIDER,
                warnings,
                unmapped_entries,
                format!("Aspect[{index}]"),
                raw_aspect.clone(),
                format!("Imported InfinityBuild build contains unknown aspect: {raw_aspect}."),
                INFINITY_BUILDS_CONVERT_ASPECT_METHOD,
            ),
        }
    }

    // Find matching rune ids (no upstream warning on a miss; empty removed).
    let mut item_runes = Vec::new();
    for raw_rune in &variant.runes {
        let rune_id = infinity_builds_convert_item_rune(lookups, raw_rune);
        if rune_id.is_empty() {
            continue;
        }
        item_runes.push(NonAffixRequirement {
            id: rune_id,
            item_type: None,
            color: colors.runes.clone(),
            kind: RuleNonAffixKind::Rune,
        });
    }

    // Find matching unique ids (ExtractOne always matches; no warning).
    let mut item_uniques = Vec::new();
    for raw_unique in &variant.uniques {
        let unique_id = lookups.match_unique(raw_unique);
        if unique_id.is_empty() {
            continue;
        }
        item_uniques.push(NonAffixRequirement {
            id: unique_id,
            item_type: None,
            color: colors.uniques.clone(),
            kind: RuleNonAffixKind::Unique,
        });
    }

    variant.affix_preset = affix_preset;
    variant.item_aspects = item_aspects;
    variant.item_uniques = item_uniques;
    variant.item_runes = item_runes;
}

// -- Public pipeline. --------------------------------------------------------

/// Full share-page import: parse the embedded JSON, export the raw variants
/// (`ExportBuildVariant`) and convert them (`ConvertBuildVariants`). The
/// returned build is ready for `export_infinity_builds_build_cache`. The
/// paragon export follows `IsImportParagonInfinityBuildsEnabled`.
pub fn import_infinity_builds_share_html(
    repo: &AssetRepository,
    html: &str,
    build_url: &str,
    settings: &AppSettings,
) -> Result<InfinityBuildsShareImport, String> {
    let build_json = parse_infinity_builds_share_html(html)?;
    let lookups = InfinityBuildsLookupTables::load(repo, INFINITY_BUILDS_LANGUAGE)?;
    let mut warnings = Vec::new();
    let mut unmapped_entries = Vec::new();

    let mut build = InfinityBuildsBuild {
        date: build_json.updated_at.clone(),
        id: build_json.share_slug.clone(),
        name: build_json.title.clone(),
        url: build_url.to_string(),
        variants: Vec::new(),
    };

    for variant_json in &build_json.variants {
        let mut variant = infinity_builds_export_variant(variant_json);
        if settings.is_import_paragon_infinity_builds_enabled {
            variant.paragon_boards = infinity_builds_paragon_boards(variant_json, &lookups)?;
        }
        infinity_builds_convert_variant(
            &mut variant,
            &lookups,
            &ImportColors::from(settings),
            &mut warnings,
            &mut unmapped_entries,
        );
        build.variants.push(variant);
    }

    Ok(InfinityBuildsShareImport {
        build,
        warnings,
        unmapped_entries,
    })
}

/// Local JSON cache write. Mirrors the upstream save in `ParseJsonBuild`
/// (`Builds/InfinityBuilds/{Id}.json`, indented JSON).
pub fn export_infinity_builds_build_cache(
    build: &InfinityBuildsBuild,
    builds_dir: &Path,
) -> Result<PathBuf, String> {
    // The build id becomes a file name: a hostile stored id (e.g. from a
    // captured share page) must never escape the builds directory.
    ensure_cache_filename_safe(&build.id)?;
    fs::create_dir_all(builds_dir)
        .map_err(|error| format!("failed to create {}: {error}", builds_dir.display()))?;
    let path = builds_dir.join(format!("{}.json", build.id));
    let text = serde_json::to_string_pretty(build)
        .map_err(|error| format!("failed to serialize InfinityBuilds build: {error}"))?;
    fs::write(&path, text).map_err(|error| {
        format!(
            "{INFINITY_BUILDS_EXPORT_VARIANT_METHOD}: failed to write {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

fn parse_infinity_builds_cache_file(path: &Path) -> Result<InfinityBuildsBuild, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

/// Mirrors upstream `LoadAvailableInfinityBuildsBuilds`: enumerate the
/// cached build JSON files and list their variants.
pub fn load_available_infinity_builds_variants(
    builds_dir: &Path,
) -> Result<Vec<ImportVariantDescriptor>, String> {
    let mut variants = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let build = parse_infinity_builds_cache_file(&path)?;
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

/// Mirrors upstream `LoadAvailableInfinityBuildsBuilds`: the cached
/// builds enumerated as rows (name, cached `UpdatedAt`, variant names).
pub fn load_available_infinity_builds_builds(
    builds_dir: &Path,
) -> Result<Vec<ImportBuildDescriptor>, String> {
    let mut builds = Vec::new();
    for path in sorted_json_files(builds_dir)? {
        let build = parse_infinity_builds_cache_file(&path)?;
        builds.push(ImportBuildDescriptor {
            variant_names: build.variants.iter().map(|v| v.name.clone()).collect(),
            id: build.id,
            name: build.name,
            url: build.url,
            date: build.date,
            source_path: path.display().to_string(),
        });
    }
    builds.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(builds)
}

fn select_infinity_builds_variant<'a>(
    build: &'a InfinityBuildsBuild,
    variant_name: Option<&str>,
) -> Result<&'a InfinityBuildsBuildVariant, String> {
    if let Some(variant_name) = variant_name {
        return build
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "missing InfinityBuilds variant '{variant_name}' in build '{}'",
                    build.id
                )
            });
    }
    build
        .variants
        .first()
        .ok_or_else(|| format!("build '{}' has no variants", build.id))
}

/// Import a variant from the local InfinityBuilds cache. Mirrors upstream
/// `CreatePresetFromInfinityBuildsBuild`: the cached conversion (produced at
/// download time by `import_infinity_builds_share_html`) is reused as-is, so
/// no asset lookups run here.
pub fn import_infinity_builds_build_variant(
    _repo: &AssetRepository,
    source_path: &Path,
    variant_name: Option<&str>,
    _settings: &AppSettings,
) -> Result<ImportWorkflowResult, String> {
    let build = parse_infinity_builds_cache_file(source_path)?;
    let source_build_id = build.id.clone();
    let source_build_name = build.name.clone();
    let variant = select_infinity_builds_variant(&build, variant_name)?;
    let preset_name = if variant.name.trim().is_empty() {
        build.name.clone()
    } else {
        variant.name.clone()
    };

    // Items grouped per item type from the cached affix preset.
    let mut items_by_type = BTreeMap::<String, Vec<AffixRequirement>>::new();
    for affix in &variant.affix_preset.affixes {
        items_by_type
            .entry(affix.item_type.clone())
            .or_default()
            .push(affix.clone());
    }
    let mut items: Vec<ImportedItemAffixes> = items_by_type
        .into_iter()
        .map(|(item_type, affixes)| ImportedItemAffixes { item_type, affixes })
        .collect();
    items.sort_by(|left, right| left.item_type.cmp(&right.item_type));

    let affix_preset = AffixPreset {
        name: preset_name.clone(),
        affixes: variant.affix_preset.affixes.clone(),
    };

    let paragon_routes = if variant.paragon_boards.is_empty() {
        Vec::new()
    } else {
        vec![ImportedParagonRoute {
            step_name: None,
            boards: variant.paragon_boards.clone(),
        }]
    };
    let paragon_boards = flatten_paragon_routes(&paragon_routes);

    let mut source_trace = vec![
        INFINITY_BUILDS_LOAD_AVAILABLE_METHOD.to_string(),
        INFINITY_BUILDS_CREATE_PRESET_METHOD.to_string(),
    ];
    if !paragon_boards.is_empty() {
        source_trace.push(INFINITY_BUILDS_PARAGON_METHOD.to_string());
    }

    Ok(ImportWorkflowResult {
        provider: INFINITY_BUILDS_PROVIDER.to_string(),
        source_url_or_file: source_path.display().to_string(),
        source_build_id,
        source_build_name,
        class_name: None,
        variant_name: Some(variant.name.clone()),
        affix_preset_name: preset_name.clone(),
        items,
        aspects: variant.item_aspects.clone(),
        uniques: variant.item_uniques.clone(),
        runes: variant.item_runes.clone(),
        affix_preset,
        non_affix_preset: NonAffixPreset {
            name: preset_name,
            powers: variant
                .item_aspects
                .iter()
                .chain(variant.item_uniques.iter())
                .cloned()
                .collect(),
            runes: variant.item_runes.clone(),
            sigils: Vec::new(),
        },
        paragon_boards,
        paragon_routes,
        // Conversion warnings are raised once at share-import time (upstream
        // `WarningOccurredMessage`s); the cache import path is silent.
        warnings: Vec::new(),
        unmapped_entries: Vec::new(),
        source_trace,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
