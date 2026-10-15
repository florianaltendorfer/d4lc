//! The smaller upstream config files: trade items, controllers, sigil
//! dungeon tiers and minimal affix values.

use crate::persistence;
use crate::{
    json, CONTROLLERS_PATH, DUNGEON_TIERS_PATH, MINIMAL_AFFIX_VALUES_PATH, TRADE_ITEMS_PATH,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Trade items (`Config/TradeItems.json`)
//
// Upstream reference: `D4Companion.Services/TradeItemManager.cs:77-107`
// (`LoadTradeItems` / `SaveTradeItems`) and
// `D4Companion.Entities/TradeItem.cs:4-15`. The upstream file is a
// top-level JSON array of `TradeItem { Affixes: List<ItemAffix>,
// Type: TradeItemType { Image, Name, Type }, Value: string }`.
//
// d4lc models the file fields the trade screen uses: the item type
// (`Type.Type`, the string `FindTradeItem` compares), the value text
// (`Value`), and the per-affix match requirements (`Affixes[].Id` +
// `Affixes[].IsGreater`). Deferred upstream fields (`Type.Image`,
// `Type.Name`, `ItemAffix.Type/Color/IsImplicit/IsTempered`) are
// deliberately not written; deserialization backfills them with the
// entity defaults, so the d4lc-written file stays loadable by upstream
// and an upstream-written file stays loadable here.
//
// `TradeItemFile.trade_item_id` is NOT an upstream file field: upstream
// does not persist an id either. d4lc synthesizes it on load —
// `Trade_<sanitized item type>`, disambiguated `_2`, `_3`, ... in file
// order — with the same rule the trade screen uses when the user adds
// a new wanted item, so a save/load round trip keeps stable ids.
// ---------------------------------------------------------------------------

/// One upstream `TradeItem` file entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TradeItemFile {
    /// Not an upstream file field; synthesized on load — see the
    /// section comment above.
    pub trade_item_id: String,
    /// Upstream `Type.Type` — the item-type string the trade lookup
    /// compares against the parsed tooltip.
    pub item_type: String,
    /// Upstream `Value` — the user-entered price text the overlay
    /// panel renders.
    pub value_text: String,
    /// Upstream `Affixes` — every entry must be present on the parsed
    /// item for the trade item to match.
    pub affixes: Vec<TradeItemFileAffix>,
}

/// The trade-relevant projection of one upstream `ItemAffix` entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TradeItemFileAffix {
    /// Upstream `Id` (`AffixInfo.IdName`).
    pub affix_id: String,
    /// Upstream `IsGreater` — when set, the parsed affix must come
    /// from a Greater affix area to match.
    pub is_greater: bool,
}

/// Load the sigil dungeon tiers from `Config/DungeonTiers.json`.
///
/// Upstream: `AffixManager.InitSigilDungeonTierData`
/// (AffixManager.cs:405-424) deserializes a flat `Dictionary<string,
/// string>` of `<sigilId, tier>`; `GetSigilDungeonTier`
/// (AffixManager.cs:744-747) returns the mapped tier for a matched
/// sigil and the caller draws it on the dungeon square marker
/// (OverlayHandler.cs:284-292) while the persisted `DungeonTiers`
/// setting is on. A missing file yields the empty map (the
/// upstream `catch` falls back to an empty dictionary too); a file
/// that exists but cannot be read or parsed is an error the caller
/// surfaces instead of silently drawing with no tiers.
pub fn load_dungeon_tiers(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = root.join(DUNGEON_TIERS_PATH);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }

    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_string_map(&text, DUNGEON_TIERS_PATH)
}

/// Load the per-affix minimal values from `Config/MinimalAffixValues.json`.
///
/// Upstream: `AffixManager.InitAffixMinimalValueData`
/// (AffixManager.cs:329-351) deserializes a flat `Dictionary<string,
/// double>` of `<affixId, minimalValue>`; `GetAffixMinimalValue`
/// (AffixManager.cs:610-613) defaults a missing id to `0`, and the
/// below-minimal comparison drives the rectangle marker shape
/// (OverlayHandler.cs:311-315). A missing file yields the empty
/// map; a malformed file is an error the caller surfaces.
pub fn load_affix_minimal_values(root: &Path) -> Result<BTreeMap<String, f64>, String> {
    let path = root.join(MINIMAL_AFFIX_VALUES_PATH);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }

    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_number_map(&text, MINIMAL_AFFIX_VALUES_PATH)
}

/// Persist the per-affix minimal values to `Config/MinimalAffixValues.json`.
///
/// Upstream: `AffixManager.SaveAffixMinimalValueData`
/// (AffixManager.cs:1058-1067) — create the `Config/` directory when
/// missing, then serialize the flat `Dictionary<string, double>` with
/// `WriteIndented = true`. Callers mirror the upstream trigger points:
/// `SetAffixMinimalValue` (AffixManager.cs:1087-1092) saves on every edit
/// and `ResetMinimalAffixValues` (AffixManager.cs:892-896) clears the map
/// and saves the emptied file. Non-finite thresholds are skipped — .NET's
/// serializer refuses `NaN`/`Infinity`, so a skipped entry matches the
/// upstream failure mode without writing an unloadable file.
pub fn save_affix_minimal_values(
    root: &Path,
    minimal_values: &BTreeMap<String, f64>,
) -> Result<(), String> {
    let json = number_map_json(minimal_values);
    write_config_text(root, MINIMAL_AFFIX_VALUES_PATH, &json)
}

/// Persist the sigil dungeon tiers to `Config/DungeonTiers.json`.
///
/// Upstream: `AffixManager.SaveSigilDungeonTierData` (AffixManager.cs:1069-1085)
/// — `SetSigilDungeonTier` (AffixManager.cs:1094-1099) writes the whole
/// map on every tier change, including the per-entry tier cycled from the
/// sigil surface.
pub fn save_dungeon_tiers(
    root: &Path,
    dungeon_tiers: &BTreeMap<String, String>,
) -> Result<(), String> {
    let json = string_map_json(dungeon_tiers);
    write_config_text(root, DUNGEON_TIERS_PATH, &json)
}

/// Write one text file under `root`, creating the `Config/` directory
/// first — the shared body of the map savers above
/// (`Directory.CreateDirectory` + `File.Create` in the upstream save
/// methods). The bytes are staged, synced and atomically renamed, so a
/// failure leaves the previous file in place.
fn write_config_text(root: &Path, relative_path: &str, text: &str) -> Result<(), String> {
    persistence::write_atomic(root, relative_path, text)
        .map(|_| ())
        .map_err(String::from)
}

/// Load the wanted trade items from `Config/TradeItems.json`.
///
/// Upstream: `TradeItemManager.LoadTradeItems` (TradeItemManager.cs:77-89).
/// A missing file yields the empty list (a fresh install starts
/// with no wanted items; upstream's loader behaves the same before its
/// unconditional save creates the file). An unreadable or malformed
/// file is an error — the caller surfaces it and keeps the in-memory
/// state instead of silently starting from data it could not read.
pub fn load_trade_items(root: &Path) -> Result<Vec<TradeItemFile>, String> {
    let path = root.join(TRADE_ITEMS_PATH);
    if !path.is_file() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_trade_items(&text)
}

/// Persist the wanted trade items to `Config/TradeItems.json`.
///
/// Upstream: `TradeItemManager.SaveTradeItems` (TradeItemManager.cs:91-100)
/// — create the `Config/` directory when missing, then write the
/// indented JSON array. Callers mirror the upstream `SaveTradeItems`
/// trigger points: every wanted-item add/edit/remove saves.
pub fn save_trade_items(root: &Path, items: &[TradeItemFile]) -> Result<(), String> {
    write_config_text(root, TRADE_ITEMS_PATH, &trade_items_json(items))
}

pub fn load_controller_config(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(CONTROLLERS_PATH);
    if !path.is_file() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_controller_config(&text)
}

/// Load the controller list the way upstream does when it starts and writes
/// it back (`SystemPresetManager.LoadControllerConfig`,
/// SystemPresetManager.cs:132-143): the first run creates
/// `Config/Controllers.json` as an empty list.
///
/// # Errors
///
/// Returns the read, parse or write error; an unreadable file is never
/// replaced by an empty list.
pub fn load_controller_config_and_save(root: &Path) -> Result<Vec<String>, String> {
    let names = load_controller_config(root)?;
    save_controller_config(root, &names)?;
    Ok(names)
}

/// Persist the active controller image list to
/// `Config/Controllers.json`.
///
/// The file is the upstream shape — an indented JSON array of the active
/// controller tooltip image filenames (`SystemPresetManager.cs:144-153`,
/// `JsonSerializer.Serialize(stream, ControllerConfig, WriteIndented)`).
/// Entries are de-duplicated case-insensitively with the first occurrence
/// kept, mirroring the upstream `AddController` guard
/// (`SystemPresetManager.cs:89-97`); the caller's order is preserved
/// because the upstream list is append-ordered.
///
/// # Errors
///
/// Returns the write error string so the caller surfaces a failed
/// selection instead of losing it.
pub fn save_controller_config(root: &Path, names: &[String]) -> Result<(), String> {
    let mut deduped: Vec<&String> = Vec::new();
    for name in names {
        if !deduped
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(name.as_str()))
        {
            deduped.push(name);
        }
    }
    write_config_text(root, CONTROLLERS_PATH, &controller_config_json(&deduped))
}

/// Render the upstream-shaped indented JSON array
/// (`System.Text.Json` with `WriteIndented = true`).
fn controller_config_json(names: &[&String]) -> String {
    json::to_file_text(&names)
}

fn parse_controller_config(text: &str) -> Result<Vec<String>, String> {
    parse_json_string_array(text)
}

fn parse_trade_items(text: &str) -> Result<Vec<TradeItemFile>, String> {
    // Upstream writes the trade-item list as a top-level JSON array
    // (`JsonSerializer.Serialize(stream, TradeItems)`).
    let items = json::parse_array(text, TRADE_ITEMS_PATH)?;
    let mut parsed: Vec<TradeItemFile> = Vec::new();
    for fields in items
        .iter()
        .filter_map(|item| json::Fields::of(item, TRADE_ITEMS_PATH))
    {
        let mut item = parse_trade_item(fields);
        item.trade_item_id = synthesize_trade_item_id(&parsed, &item.item_type);
        parsed.push(item);
    }
    Ok(parsed)
}

/// Parse one trade-item object. Missing upstream fields fall back to
/// the entity defaults (`Value` → empty string, `Type.Type` → empty
/// string, `IsGreater` → false), matching what .NET deserialization
/// would produce for a d4lc-written file.
fn parse_trade_item(fields: json::Fields<'_>) -> TradeItemFile {
    TradeItemFile {
        trade_item_id: String::new(),
        item_type: fields
            .value("Type")
            .and_then(|value| json::Fields::of(value, TRADE_ITEMS_PATH))
            .and_then(|item_type| item_type.string("Type"))
            .unwrap_or_default(),
        value_text: fields.string("Value").unwrap_or_default(),
        affixes: fields
            .objects("Affixes")
            .unwrap_or_default()
            .into_iter()
            .map(|affix| TradeItemFileAffix {
                affix_id: affix.string("Id").unwrap_or_default(),
                is_greater: affix.bool("IsGreater").unwrap_or(false),
            })
            .collect(),
    }
}

/// Synthesize the d4lc-local `trade_item_id` for a loaded entry:
/// `Trade_<sanitized item type>`, disambiguated `_2`, `_3`, ... in
/// file order. Same rule as the trade screen's id generation, so a
/// save/load round trip of the in-memory state keeps stable ids.
fn synthesize_trade_item_id(existing: &[TradeItemFile], item_type: &str) -> String {
    let mut sanitized: String = item_type
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        sanitized.push_str("Item");
    }
    let base = format!("Trade_{sanitized}");
    if !existing.iter().any(|item| item.trade_item_id == base) {
        return base;
    }
    for n in 2..u32::MAX {
        let candidate = format!("{base}_{n}");
        if !existing.iter().any(|item| item.trade_item_id == candidate) {
            return candidate;
        }
    }
    base
}

fn parse_json_string_array(text: &str) -> Result<Vec<String>, String> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("Config/Controllers.json must be a JSON string array: {error}"))?;
    let Value::Array(items) = document else {
        return Err("Config/Controllers.json must be a JSON string array".to_string());
    };
    let mut values = Vec::new();
    for item in items {
        match item {
            Value::String(value) => values.push(value),
            _ => return Err("Config/Controllers.json must contain strings only".to_string()),
        }
    }
    values.sort_by_key(|left| left.to_ascii_lowercase());
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(values)
}

/// Parse a flat JSON object of `<string, string>` entries (the upstream
/// `Dictionary<string, string>` serialization shape of
/// `Config/DungeonTiers.json`). Non-string values are skipped — the
/// loadable subset is kept rather than rejecting the whole map.
fn parse_string_map(text: &str, source: &str) -> Result<BTreeMap<String, String>, String> {
    Ok(json::parse_object(text, source)?
        .into_iter()
        .filter_map(|(key, value)| match value {
            Value::String(value) => Some((key, value)),
            _ => None,
        })
        .collect())
}

/// Parse a flat JSON object of `<string, number>` entries (the upstream
/// `Dictionary<string, double>` serialization shape of
/// `Config/MinimalAffixValues.json`). Entries without a number are
/// skipped; see [`parse_string_map`].
fn parse_number_map(text: &str, source: &str) -> Result<BTreeMap<String, f64>, String> {
    let object = json::parse_object(text, source)?;
    let fields = json::Fields::new(&object, source);
    Ok(object
        .keys()
        .filter_map(|key| fields.float(key).map(|value| (key.clone(), value)))
        .collect())
}

/// Serialize a flat `<string, string>` map (the `DungeonTiers.json`
/// shape) as the indented JSON object [`save_dungeon_tiers`] writes.
fn string_map_json(values: &BTreeMap<String, String>) -> String {
    json::to_file_text(values)
}

/// Serialize a flat `<string, double>` map (the `MinimalAffixValues.json`
/// shape) as the indented JSON object [`save_affix_minimal_values`]
/// writes. Non-finite thresholds are skipped: .NET's serializer refuses
/// `NaN`/`Infinity`, so dropping the entry matches the upstream failure
/// mode instead of writing an unloadable file.
fn number_map_json(values: &BTreeMap<String, f64>) -> String {
    json::to_file_text(
        &values
            .iter()
            .filter(|(_, value)| value.is_finite())
            .map(|(key, value)| (key, json::number(*value)))
            .collect::<BTreeMap<_, _>>(),
    )
}

/// Serialize the trade items as the upstream-shaped indented JSON
/// array (upstream `SaveTradeItems` uses `WriteIndented = true`).
///
/// The keys mirror `TradeItem.cs` / `ItemAffix.cs`; fields the port does
/// not model (`Type.Image`, `Type.Name`,
/// `ItemAffix.Type/Color/IsImplicit/IsTempered`) are not written, and
/// neither is the d4lc-local `trade_item_id`: upstream persists no id.
fn trade_items_json(items: &[TradeItemFile]) -> String {
    json::to_file_text(
        &items
            .iter()
            .map(|item| {
                json::Object::new()
                    .field(
                        "Affixes",
                        item.affixes
                            .iter()
                            .map(|affix| {
                                json::Object::new()
                                    .field("Id", &affix.affix_id)
                                    .field("IsGreater", affix.is_greater)
                            })
                            .collect::<Vec<_>>(),
                    )
                    .field("Type", json::Object::new().field("Type", &item.item_type))
                    .field("Value", &item.value_text)
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::{create_file, create_temp_root, remove_dir};
    use std::fs;

    #[test]
    fn startup_load_creates_the_empty_controller_file_and_keeps_an_existing_list() {
        // SystemPresetManager.cs:132-143: LoadControllerConfig ends with
        // SaveControllerConfig.
        let root = create_temp_root("startup_load_creates_the_empty_controller_file");
        assert_eq!(
            load_controller_config_and_save(&root).expect("fresh install"),
            Vec::<String>::new()
        );
        assert_eq!(
            fs::read_to_string(root.join(CONTROLLERS_PATH)).expect("created file"),
            "[]\n"
        );

        create_file(&root.join(CONTROLLERS_PATH), r#"["tooltip_gc_x.png"]"#);
        assert_eq!(
            load_controller_config_and_save(&root).expect("existing list"),
            ["tooltip_gc_x.png"]
        );
        assert_eq!(
            fs::read_to_string(root.join(CONTROLLERS_PATH)).expect("rewritten file"),
            "[\n  \"tooltip_gc_x.png\"\n]\n"
        );

        create_file(&root.join(CONTROLLERS_PATH), "{ broken");
        assert!(load_controller_config_and_save(&root).is_err());
        assert_eq!(
            fs::read_to_string(root.join(CONTROLLERS_PATH)).expect("untouched"),
            "{ broken"
        );
        remove_dir(&root);
    }

    #[test]
    fn save_controller_config_writes_the_upstream_string_array() {
        // Upstream SystemPresetManager.cs:144-153 writes an indented
        // JSON array of the active controller image filenames; the list
        // is append-ordered and AddController guards duplicates
        // case-insensitively (SystemPresetManager.cs:89-97).
        let root = create_temp_root("save_controller_config_writes_the_upstream_string_array");
        let names = vec![
            "tooltip_gc_enus_amulet_0.png".to_string(),
            "tooltip_gc_enus_boots_1.png".to_string(),
            "TOOLTIP_GC_ENUS_AMULET_0.PNG".to_string(),
        ];
        save_controller_config(&root, &names)
            .unwrap_or_else(|error| panic!("controller config should save: {error}"));

        let raw = std::fs::read_to_string(root.join(CONTROLLERS_PATH))
            .unwrap_or_else(|error| panic!("read Controllers.json: {error}"));
        assert_eq!(
            raw,
            "[\n  \"tooltip_gc_enus_amulet_0.png\",\n  \"tooltip_gc_enus_boots_1.png\"\n]\n"
        );

        // The reader accepts what the writer produced and keeps the
        // documented sort + case-insensitive de-duplication.
        let reloaded = load_controller_config(&root)
            .unwrap_or_else(|error| panic!("controller config should reload: {error}"));
        assert_eq!(
            reloaded,
            vec![
                "tooltip_gc_enus_amulet_0.png".to_string(),
                "tooltip_gc_enus_boots_1.png".to_string()
            ]
        );

        // An empty selection clears the file to the empty array.
        save_controller_config(&root, &[])
            .unwrap_or_else(|error| panic!("empty controller config should save: {error}"));
        assert!(load_controller_config(&root)
            .unwrap_or_else(|error| panic!("empty controller config should reload: {error}"))
            .is_empty());

        remove_dir(&root);
    }

    #[test]
    fn load_dungeon_tiers_reads_flat_string_map() {
        // Upstream serializes `_sigilDungeonTiers` as a flat
        // Dictionary<string, string> (AffixManager.cs:1066-1077).
        let root = create_temp_root("load_dungeon_tiers_reads_flat_string_map");
        let config_dir = root.join("Config");
        std::fs::create_dir_all(&config_dir)
            .unwrap_or_else(|error| panic!("create Config dir: {error}"));
        std::fs::write(
            config_dir.join("DungeonTiers.json"),
            r#"{
  "World_DGN_Kehj_AbandonedMineworks": "S2",
  "World_DGN_Scos_MoorthanMine": "T30"
}"#,
        )
        .unwrap_or_else(|error| panic!("write DungeonTiers.json: {error}"));

        let tiers = load_dungeon_tiers(&root)
            .unwrap_or_else(|error| panic!("dungeon tiers should load: {error}"));
        assert_eq!(
            tiers
                .get("World_DGN_Kehj_AbandonedMineworks")
                .map(String::as_str),
            Some("S2")
        );
        assert_eq!(
            tiers.get("World_DGN_Scos_MoorthanMine").map(String::as_str),
            Some("T30")
        );
        assert_eq!(tiers.len(), 2);

        remove_dir(&root);
    }

    #[test]
    fn load_dungeon_tiers_missing_file_yields_empty_map() {
        let root = create_temp_root("load_dungeon_tiers_missing_file_yields_empty_map");
        let tiers = load_dungeon_tiers(&root)
            .unwrap_or_else(|error| panic!("missing file must stay an empty map: {error}"));
        assert!(tiers.is_empty());
        remove_dir(&root);
    }

    #[test]
    fn save_dungeon_tiers_round_trips_through_the_loader() {
        // Upstream writes the whole map on every tier change
        // (`SaveSigilDungeonTierData`, AffixManager.cs:1069-1085).
        let root = create_temp_root("save_dungeon_tiers_round_trips_through_the_loader");
        let mut tiers = BTreeMap::new();
        tiers.insert(
            "World_DGN_Kehj_AbandonedMineworks".to_string(),
            "S2".to_string(),
        );
        tiers.insert("World_DGN_Scos_MoorthanMine".to_string(), "T30".to_string());

        save_dungeon_tiers(&root, &tiers)
            .unwrap_or_else(|error| panic!("tiers should save: {error}"));

        let text = fs::read_to_string(root.join(DUNGEON_TIERS_PATH))
            .unwrap_or_else(|error| panic!("read saved tiers: {error}"));
        assert_eq!(
            text,
            "{\n  \"World_DGN_Kehj_AbandonedMineworks\": \"S2\",\n  \
             \"World_DGN_Scos_MoorthanMine\": \"T30\"\n}\n"
        );
        let reloaded = load_dungeon_tiers(&root)
            .unwrap_or_else(|error| panic!("saved tiers should reload: {error}"));
        assert_eq!(reloaded, tiers);

        remove_dir(&root);
    }

    #[test]
    fn save_dungeon_tiers_creates_the_config_directory() {
        // The upstream save methods call Directory.CreateDirectory, so a
        // fresh config root persists instead of failing the write.
        let root = create_temp_root("save_dungeon_tiers_creates_the_config_directory");
        let mut tiers = BTreeMap::new();
        tiers.insert("World_DGN_Test".to_string(), "A".to_string());
        save_dungeon_tiers(&root, &tiers)
            .unwrap_or_else(|error| panic!("tiers should save into a fresh root: {error}"));
        assert!(root.join(DUNGEON_TIERS_PATH).is_file());
        remove_dir(&root);
    }

    #[test]
    fn save_affix_minimal_values_round_trips_and_writes_the_reset_shape() {
        // `SetAffixMinimalValue` saves on every edit
        // (AffixManager.cs:1087-1092); `ResetMinimalAffixValues`
        // (AffixManager.cs:892-896) clears the map and saves the emptied
        // file — the `{}` shape asserted here.
        let root =
            create_temp_root("save_affix_minimal_values_round_trips_and_writes_the_reset_shape");
        let mut values = BTreeMap::new();
        values.insert("Affix_AttackSpeed".to_string(), 12.5_f64);
        values.insert("Affix_CritDamage".to_string(), 40.0_f64);

        save_affix_minimal_values(&root, &values)
            .unwrap_or_else(|error| panic!("minimal values should save: {error}"));
        let text = fs::read_to_string(root.join(MINIMAL_AFFIX_VALUES_PATH))
            .unwrap_or_else(|error| panic!("read saved minimal values: {error}"));
        assert_eq!(
            text,
            "{\n  \"Affix_AttackSpeed\": 12.5,\n  \"Affix_CritDamage\": 40\n}\n"
        );
        assert_eq!(
            load_affix_minimal_values(&root)
                .unwrap_or_else(|error| panic!("saved minimal values should reload: {error}")),
            values
        );

        save_affix_minimal_values(&root, &BTreeMap::new())
            .unwrap_or_else(|error| panic!("reset save should succeed: {error}"));
        let reset_text = fs::read_to_string(root.join(MINIMAL_AFFIX_VALUES_PATH))
            .unwrap_or_else(|error| panic!("read reset minimal values: {error}"));
        assert_eq!(reset_text, "{}\n");
        assert!(load_affix_minimal_values(&root)
            .unwrap_or_else(|error| panic!("reset file should reload: {error}"))
            .is_empty());

        remove_dir(&root);
    }

    #[test]
    fn save_affix_minimal_values_skips_non_finite_thresholds() {
        // .NET's serializer refuses NaN/Infinity; skipping the entry
        // keeps the file loadable instead of writing invalid JSON.
        let root = create_temp_root("save_affix_minimal_values_skips_non_finite_thresholds");
        let mut values = BTreeMap::new();
        values.insert("Affix_AttackSpeed".to_string(), 5.0_f64);
        values.insert("Affix_Broken".to_string(), f64::NAN);

        save_affix_minimal_values(&root, &values)
            .unwrap_or_else(|error| panic!("finite entries should save: {error}"));
        let reloaded = load_affix_minimal_values(&root)
            .unwrap_or_else(|error| panic!("saved minimal values should reload: {error}"));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.get("Affix_AttackSpeed"), Some(&5.0_f64));

        remove_dir(&root);
    }

    #[test]
    fn load_affix_minimal_values_reads_flat_number_map() {
        // Upstream serializes `_minimalAffixValues` as a flat
        // Dictionary<string, double> (AffixManager.cs:1060-1070).
        let root = create_temp_root("load_affix_minimal_values_reads_flat_number_map");
        let config_dir = root.join("Config");
        std::fs::create_dir_all(&config_dir)
            .unwrap_or_else(|error| panic!("create Config dir: {error}"));
        std::fs::write(
            config_dir.join("MinimalAffixValues.json"),
            r#"{
  "Affix_AttackSpeed": 12.5,
  "Affix_CritDamage": 40,
  "Affix_Broken": "not-a-number"
}"#,
        )
        .unwrap_or_else(|error| panic!("write MinimalAffixValues.json: {error}"));

        let minimal_values = load_affix_minimal_values(&root)
            .unwrap_or_else(|error| panic!("minimal values should load: {error}"));
        assert_eq!(minimal_values.get("Affix_AttackSpeed"), Some(&12.5));
        assert_eq!(minimal_values.get("Affix_CritDamage"), Some(&40.0));
        // Entries without a parseable number are skipped, not fatal.
        assert!(!minimal_values.contains_key("Affix_Broken"));
        assert_eq!(minimal_values.len(), 2);

        remove_dir(&root);
    }

    #[test]
    fn load_affix_minimal_values_missing_file_yields_empty_map() {
        let root = create_temp_root("load_affix_minimal_values_missing_file_yields_empty_map");
        let minimal_values = load_affix_minimal_values(&root)
            .unwrap_or_else(|error| panic!("missing file must stay an empty map: {error}"));
        assert!(minimal_values.is_empty());
        remove_dir(&root);
    }

    #[test]
    fn loads_upstream_shaped_controller_config_json() {
        let root = create_temp_root("loads_upstream_shaped_controller_config_json");
        create_file(
            &root.join(CONTROLLERS_PATH),
            r#"["tooltip_gc_x.png", "tooltip_gc_x.png", "tooltip_kb_all.png"]"#,
        );

        let controllers = load_controller_config(&root)
            .unwrap_or_else(|error| panic!("failed to load controllers: {error}"));
        assert_eq!(controllers.len(), 2);
        assert!(controllers.iter().any(|name| name == "tooltip_gc_x.png"));
        assert!(controllers.iter().any(|name| name == "tooltip_kb_all.png"));

        remove_dir(&root);
    }

    #[test]
    fn missing_trade_items_file_yields_empty_state() {
        let root = create_temp_root("missing_trade_items_file_yields_empty_state");
        let items = load_trade_items(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert!(
            items.is_empty(),
            "a fresh install starts with no trade items"
        );
        remove_dir(&root);
    }

    #[test]
    fn loads_upstream_shaped_trade_items_and_ignores_deferred_fields() {
        let root = create_temp_root("loads_upstream_shaped_trade_items");
        // Fixture in the upstream shape: the deferred fields
        // `Type.Image`, `Type.Name`, and the per-affix `Type`, `Color`,
        // `IsImplicit`, `IsTempered` are present exactly as upstream
        // writes them; they must load without error and without
        // inventing d4lc fields for them.
        create_file(
            &root.join(TRADE_ITEMS_PATH),
            r##"[
  {
    "Affixes": [
      { "Id": "Affix_MaxLife", "Type": "Helm", "Color": { "A": 255, "R": 0, "G": 128, "B": 0 }, "IsGreater": true, "IsImplicit": false, "IsTempered": false },
      { "Id": "Affix_Armor", "Type": "", "Color": {}, "IsGreater": false, "IsImplicit": true, "IsTempered": false }
    ],
    "Type": { "Image": "helm.png", "Name": "Helm display", "Type": "Helm" },
    "Value": "750k"
  }
]"##,
        );

        let items = load_trade_items(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(items.len(), 1);
        let item = &items[0];
        // The id is synthesized, not read from the file.
        assert_eq!(item.trade_item_id, "Trade_Helm");
        assert_eq!(item.item_type, "Helm");
        assert_eq!(item.value_text, "750k");
        assert_eq!(item.affixes.len(), 2);
        assert_eq!(item.affixes[0].affix_id, "Affix_MaxLife");
        assert!(item.affixes[0].is_greater);
        assert_eq!(item.affixes[1].affix_id, "Affix_Armor");
        assert!(!item.affixes[1].is_greater);

        remove_dir(&root);
    }

    #[test]
    fn trade_items_synthesize_collision_free_ids_in_file_order() {
        let root = create_temp_root("trade_items_synthesize_collision_free_ids");
        create_file(
            &root.join(TRADE_ITEMS_PATH),
            r##"[
  { "Affixes": [], "Type": { "Type": "Amulet" }, "Value": "10M" },
  { "Affixes": [], "Type": { "Type": "Two-Handed Sword" }, "Value": "5b" },
  { "Affixes": [], "Type": { "Type": "Amulet" }, "Value": "20M" }
]"##,
        );

        let items = load_trade_items(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(
            items
                .iter()
                .map(|item| item.trade_item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["Trade_Amulet", "Trade_Two_Handed_Sword", "Trade_Amulet_2"],
        );

        remove_dir(&root);
    }

    #[test]
    fn trade_items_round_trip_through_save_and_load() {
        let root = create_temp_root("trade_items_round_trip_through_save_and_load");
        let items = vec![
            TradeItemFile {
                trade_item_id: "Trade_Amulet".to_string(),
                item_type: "Amulet".to_string(),
                value_text: "10M".to_string(),
                affixes: vec![TradeItemFileAffix {
                    affix_id: "Affix_AttackSpeed".to_string(),
                    is_greater: false,
                }],
            },
            TradeItemFile {
                trade_item_id: "Trade_Helm".to_string(),
                item_type: "Helm".to_string(),
                value_text: "750k".to_string(),
                affixes: vec![
                    TradeItemFileAffix {
                        affix_id: "Affix_MaxLife".to_string(),
                        is_greater: true,
                    },
                    TradeItemFileAffix {
                        affix_id: "Affix_Armor".to_string(),
                        is_greater: false,
                    },
                ],
            },
        ];

        save_trade_items(&root, &items).unwrap_or_else(|error| panic!("save failed: {error}"));
        let reloaded =
            load_trade_items(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        // The file carries no ids, so the reload synthesizes them; with
        // the documented synthesis rule they match the in-memory ids.
        assert_eq!(reloaded, items);

        // Upstream-shape spot checks on the written bytes: upstream
        // entity keys and the top-level array (so upstream's
        // `Deserialize<List<TradeItem>>` reads the file back), and no
        // d4lc-local id key.
        let text = fs::read_to_string(root.join(TRADE_ITEMS_PATH))
            .unwrap_or_else(|error| panic!("failed to read trade items: {error}"));
        assert!(text.trim_start().starts_with('['));
        for key in [
            "\"Affixes\"",
            "\"Type\"",
            "\"Value\"",
            "\"Id\"",
            "\"IsGreater\"",
        ] {
            assert!(
                text.contains(key),
                "written file must contain {key}: {text}"
            );
        }
        assert!(!text.contains("\"trade_item_id\""));

        remove_dir(&root);
    }

    #[test]
    fn malformed_trade_items_file_is_an_error() {
        let root = create_temp_root("malformed_trade_items_file_is_an_error");
        create_file(&root.join(TRADE_ITEMS_PATH), "{ not an array");
        let result = load_trade_items(&root);
        assert!(result.is_err(), "a non-array trade-items file must fail");
        remove_dir(&root);
    }
}
