use crate::persistence::{self, CommittedRevision, ConfigError};
use crate::settings::{app_settings_json, keep_settings_migration_backup, load_settings_for_write};
use crate::{json, AFFIX_PRESETS_PATH, SETTINGS_PATH};
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Affix-preset file model (Config/AffixPresets-v2.json)
//
// Upstream reference:
// - `D4Companion.Entities/AffixPreset.cs:5-12` — preset shape.
// - `D4Companion.Entities/ItemAffix.cs:8-18` — entry shape.
// - `D4Companion.Services/AffixManager.cs:913-1058` — load/save and the
//   `SaveAffixPresets` trigger points this crate mirrors.
//
// `ItemAffix.IsAnyType` and `ItemAffix.TuningPrisms` are first-class typed
// fields of the preset model: `is_any_type` stays
// independent of `item_type` (an "any item type" entry keeps the item
// type it was imported for) and `tuning_prisms` carries the upstream
// prism-name list unchanged on load, edit and save.
//
// `AffixPreset.ParagonBoardsList` is persisted so build imports keep their
// boards across restarts. The list mirrors the upstream entity exactly (one inner list per journey
// step, each `ParagonBoard` carrying `Name`, `Glyph`, `Rotation`, the
// flattened 21×21 `Nodes` bool grid and the D2Core `Index`), so an
// upstream-written file loads unchanged and a d4lc-written file stays
// loadable by upstream. An empty list is omitted from the file, which
// upstream backfills with the entity default on load.
// ---------------------------------------------------------------------------

/// Side length of the upstream paragon board grid
/// (`ParagonBoard.Nodes = new bool[21*21]`, `ParagonBoard.cs:14`).
pub const PARAGON_GRID_EDGE: usize = 21;

/// Flattened tile count of the upstream paragon board grid.
pub const PARAGON_GRID_LEN: usize = PARAGON_GRID_EDGE * PARAGON_GRID_EDGE;

/// One upstream `AffixPreset` file entry.
///
/// The five lists mirror the upstream entity exactly; d4lc's mixed-kind
/// editor entry list maps an entry into the list named by its kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AffixPreset {
    pub name: String,
    pub item_affixes: Vec<PresetItemAffix>,
    pub item_aspects: Vec<PresetItemAffix>,
    pub item_sigils: Vec<PresetItemAffix>,
    pub item_uniques: Vec<PresetItemAffix>,
    pub item_runes: Vec<PresetItemAffix>,
    /// Upstream `ParagonBoardsList` (`AffixPreset.cs:11`): one inner
    /// list per journey step, each step holding its paragon boards in
    /// display order. Empty means the preset carries no boards, which
    /// is also the shape of every pre-paragon d4lc preset file.
    pub paragon_boards_list: Vec<Vec<PresetParagonBoard>>,
}

/// One board inside an [`AffixPreset::paragon_boards_list`] step,
/// mirroring upstream `ParagonBoard` (`ParagonBoard.cs:9-19`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetParagonBoard {
    /// Upstream `Name` — the localized board name.
    pub name: String,
    /// Upstream `Glyph` — the localized glyph name.
    pub glyph: String,
    /// Upstream `Rotation` — `"0°"`, `"90°"`, `"180°"` or `"270°"`.
    pub rotation: String,
    /// Positions of the active tiles in the flattened 21×21 grid
    /// (`x + y * 21`). d4lc's compact in-memory form of the upstream
    /// `Nodes` bool array, which is expanded at the file boundary.
    pub active_nodes: Vec<usize>,
    /// Upstream `Index` — the D2Core board ordering value. Other
    /// providers leave it at the entity default `0`; board order is
    /// carried by the list position in every case.
    pub index: i32,
}

/// One entry inside an [`AffixPreset`], mirroring upstream `ItemAffix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetItemAffix {
    /// Upstream `Id` (`AffixInfo.IdName`).
    pub id: String,
    /// Upstream `Type`; empty means "no item-type constraint".
    pub item_type: String,
    /// Upstream `Color` (`System.Windows.Media.Color`), serialized as
    /// the upstream `Color` object (see `wpf_color`).
    pub color: PresetColor,
    /// Upstream `IsAnyType`: the entry matches the affix regardless of
    /// the item type. Independent of [`Self::item_type`] — upstream
    /// keeps both the imported type and the flag, and so does d4lc.
    pub is_any_type: bool,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    /// Upstream `TuningPrisms`: the prism names the overlay draws beside
    /// a matched affix. Order is preserved; entries are not deduplicated
    /// (upstream copies the catalog list as it is).
    pub tuning_prisms: Vec<String>,
    /// d4lc-local display label (the gui editor's human-readable entry
    /// name). NOT an upstream field — upstream `ItemAffix` has no
    /// `Label`, and .NET's default deserialization skips unknown
    /// properties, so a d4lc-written file stays loadable by upstream.
    /// `None` on files without the field (every pre-label d4lc file),
    /// which keeps old files unchanged on load.
    pub label: Option<String>,
}

/// Byte-level RGBA color as serialized by upstream's WPF `Color`.
///
/// d4lc keeps entry colors as strings (`"#RRGGBB"` or a basic color
/// name); [`PresetColor::parse`] / [`PresetColor::to_string_form`]
/// convert at the file boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetColor {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl PresetColor {
    /// Upstream `Colors.Green` — the `ItemAffix.Color` initializer in
    /// `ItemAffix.cs:12` and the fallback for colors d4lc cannot parse.
    pub const GREEN: Self = Self {
        a: 255,
        r: 0,
        g: 128,
        b: 0,
    };

    /// Parse d4lc's string color form: `#RGB`, `#RRGGBB`,
    /// `#AARRGGBB`, or a basic color name. Unparseable input falls
    /// back to [`PresetColor::GREEN`] instead of inventing a value.
    pub fn parse(value: &str) -> Self {
        Self::try_parse(value).unwrap_or(Self::GREEN)
    }

    /// Strict variant of [`PresetColor::parse`]: the same accepted
    /// forms, but unparseable input is a structured error instead of
    /// the `Green` fallback.
    ///
    /// The lenient fallback is right for stored preset colors (upstream
    /// also defaults an unknown `ItemAffix.Color` to green); it is wrong
    /// for the ColorsConfig dialog, where a typo silently becoming green
    /// would hide the mistake the user just made.
    ///
    /// # Errors
    ///
    /// Returns a user-visible message naming the accepted forms.
    pub fn try_parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if let Some(hex) = value.strip_prefix('#') {
            return parse_hex_color(hex).ok_or_else(|| {
                format!("'{value}' is not a hex color; expected #RGB, #RRGGBB or #AARRGGBB")
            });
        }
        match value.to_ascii_lowercase().as_str() {
            "black" => Ok(Self::from_rgb(0, 0, 0)),
            "silver" => Ok(Self::from_rgb(192, 192, 192)),
            "gray" | "grey" => Ok(Self::from_rgb(128, 128, 128)),
            "white" => Ok(Self::from_rgb(255, 255, 255)),
            "maroon" => Ok(Self::from_rgb(128, 0, 0)),
            "red" => Ok(Self::from_rgb(255, 0, 0)),
            "purple" => Ok(Self::from_rgb(128, 0, 128)),
            "magenta" | "fuchsia" => Ok(Self::from_rgb(255, 0, 255)),
            "green" => Ok(Self::from_rgb(0, 128, 0)),
            "lime" => Ok(Self::from_rgb(0, 255, 0)),
            "olive" => Ok(Self::from_rgb(128, 128, 0)),
            "yellow" => Ok(Self::from_rgb(255, 255, 0)),
            "navy" => Ok(Self::from_rgb(0, 0, 128)),
            "blue" => Ok(Self::from_rgb(0, 0, 255)),
            "teal" => Ok(Self::from_rgb(0, 128, 128)),
            "aqua" | "cyan" => Ok(Self::from_rgb(0, 255, 255)),
            "orange" => Ok(Self::from_rgb(255, 165, 0)),
            "gold" => Ok(Self::from_rgb(255, 215, 0)),
            _ => Err(format!(
                "'{value}' is not a known color; use a hex value (#RRGGBB) or a basic color name"
            )),
        }
    }

    /// Render d4lc's string color form: `#RRGGBB`, or `#AARRGGBB`
    /// when the alpha byte is not opaque.
    pub fn to_string_form(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
        }
    }

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { a: 255, r, g, b }
    }
}

fn parse_hex_color(hex: &str) -> Option<PresetColor> {
    fn byte(value: &str) -> Option<u8> {
        u8::from_str_radix(value, 16).ok()
    }
    match hex.len() {
        3 => Some(PresetColor {
            a: 255,
            r: byte(&hex[0..1])? * 17,
            g: byte(&hex[1..2])? * 17,
            b: byte(&hex[2..3])? * 17,
        }),
        6 => Some(PresetColor {
            a: 255,
            r: byte(&hex[0..2])?,
            g: byte(&hex[2..4])?,
            b: byte(&hex[4..6])?,
        }),
        8 => Some(PresetColor {
            a: byte(&hex[0..2])?,
            r: byte(&hex[2..4])?,
            g: byte(&hex[4..6])?,
            b: byte(&hex[6..8])?,
        }),
        _ => None,
    }
}

/// Load the affix presets from `Config/AffixPresets-v2.json`.
///
/// Upstream: `AffixManager.LoadAffixPresets` (AffixManager.cs:913-926).
/// A missing file yields the empty list (a fresh install starts
/// with no presets). An unreadable or malformed file is an error — the
/// caller surfaces it and keeps the in-memory state instead of silently
/// starting from data it could not read.
pub fn load_affix_presets(root: &Path) -> Result<Vec<AffixPreset>, ConfigError> {
    let path = root.join(AFFIX_PRESETS_PATH);
    if !path.is_file() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path).map_err(ConfigError::io(&path, "read"))?;
    let mut presets = parse_affix_presets(&text)?;
    // AffixManager.cs:920-924: the loaded list is sorted by name.
    presets.sort_by(|left, right| compare_ordinal(&left.name, &right.name));
    Ok(presets)
}

/// `string.Compare(x, y, StringComparison.Ordinal)`: UTF-16 code units, so a
/// character outside the BMP orders before U+E000..U+FFFF as it does in .NET.
pub fn compare_ordinal(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// The order `AffixManager.SaveAffixPresets` leaves `ItemAffixes` in
/// (AffixManager.cs:1030-1045): non-tempered before tempered, implicit before
/// non-implicit, then by `Id`.
fn sort_item_affixes(entries: &mut [PresetItemAffix]) {
    entries.sort_by(|left, right| {
        left.is_tempered
            .cmp(&right.is_tempered)
            .then_with(|| right.is_implicit.cmp(&left.is_implicit))
            .then_with(|| compare_culture(&left.id, &right.id))
    });
}

/// `string.CompareTo`, the culture-sensitive comparison upstream orders ids
/// with, for the ASCII identifiers the catalogs use: punctuation sorts before
/// digits and digits before letters, letters compare without regard to case
/// first, and a lowercase letter sorts before its uppercase form when
/// nothing else differs. Characters outside ASCII compare by lowercase code
/// point.
fn compare_culture(left: &str, right: &str) -> std::cmp::Ordering {
    // CLDR root order of the ASCII punctuation and symbols.
    const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    fn primary(ch: char) -> (u8, u32) {
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        if lower.is_ascii_digit() {
            (1, u32::from(lower))
        } else if lower.is_alphabetic() {
            (2, u32::from(lower))
        } else {
            let rank = PUNCTUATION
                .find(lower)
                .map_or(u32::from(lower), |at| at as u32);
            (0, rank)
        }
    }
    left.chars()
        .map(primary)
        .cmp(right.chars().map(primary))
        .then_with(|| {
            // Tertiary difference: lowercase before uppercase.
            left.chars()
                .zip(right.chars())
                .find(|(l, r)| l != r)
                .map_or(std::cmp::Ordering::Equal, |(l, r)| {
                    r.is_lowercase().cmp(&l.is_lowercase())
                })
        })
}

/// Persist the affix presets to `Config/AffixPresets-v2.json`.
///
/// Upstream: `AffixManager.SaveAffixPresets` (AffixManager.cs:1049-1058)
/// — create the `Config/` directory when missing, then write the
/// indented JSON array. Callers mirror the upstream `SaveAffixPresets`
/// trigger points: every preset and preset-entry mutation saves.
///
/// The write is staged, synced and atomically renamed; the returned
/// revision describes exactly the bytes that reached the disk.
///
/// # Errors
///
/// Returns the write error — including [`ConfigError::PendingRecovery`]
/// when the file belongs to an interrupted change — so the caller can
/// keep the previously applied revision and report the failure.
pub fn save_affix_presets(
    root: &Path,
    presets: &[AffixPreset],
) -> Result<CommittedRevision, ConfigError> {
    ensure_existing_presets_readable(root)?;
    persistence::write_atomic(root, AFFIX_PRESETS_PATH, &affix_presets_json(presets))
}

/// [`save_affix_presets`], except that nothing is written when the file
/// already holds exactly the bytes the save would produce. `None` then.
///
/// The startup load-and-save of `AffixManager.LoadAffixPresets`
/// (AffixManager.cs:926) uses it: an up-to-date file is neither rewritten nor
/// re-timestamped.
///
/// # Errors
///
/// Same as [`save_affix_presets`].
pub fn save_affix_presets_if_changed(
    root: &Path,
    presets: &[AffixPreset],
) -> Result<Option<CommittedRevision>, ConfigError> {
    let path = root.join(AFFIX_PRESETS_PATH);
    if fs::read_to_string(&path).is_ok_and(|current| current == affix_presets_json(presets)) {
        return Ok(None);
    }
    save_affix_presets(root, presets).map(Some)
}

/// Keep a copy of the stored preset file before a migration rewrites it.
///
/// `label` names the migration (`pre-id-migration`); the copy lands at
/// `Config/AffixPresets-v2.{label}.bak.json`, is written once and never
/// overwritten, so the first pre-migration state stays restorable by a
/// file copy. Returns the relative backup path, or `None` when there is
/// no stored preset file to keep.
///
/// # Errors
///
/// Returns the read or write error; the caller must not rewrite the
/// preset file without its backup.
pub fn keep_affix_presets_migration_backup(
    root: &Path,
    label: &str,
) -> Result<Option<String>, ConfigError> {
    let path = root.join(AFFIX_PRESETS_PATH);
    if !path.is_file() {
        return Ok(None);
    }
    let backup = format!("Config/AffixPresets-v2.{label}.bak.json");
    if root.join(&backup).exists() {
        return Ok(Some(backup));
    }
    let text = fs::read_to_string(&path).map_err(ConfigError::io(&path, "read"))?;
    persistence::write_atomic(root, &backup, &text)?;
    Ok(Some(backup))
}

/// Refuse to replace a preset file this build cannot read.
///
/// Overwriting user data we could not parse would silently discard it
/// (the file may come from a newer shape or a hand-edited file); the
/// caller keeps the in-memory state and surfaces the read error instead.
fn ensure_existing_presets_readable(root: &Path) -> Result<(), ConfigError> {
    if !root.join(AFFIX_PRESETS_PATH).is_file() {
        return Ok(());
    }
    load_affix_presets(root).map(|_| ())
}

/// Persist the affix presets and the selected preset name as one
/// transaction — the upstream `SaveAffixPresets` + `SaveSettings`
/// sequence of a preset import (`AffixManager.AddAffixPreset`).
///
/// Both files are staged and recorded before either is replaced, so an
/// interrupted change is visible at the next start instead of leaving a
/// preset file and a selection that silently disagree.
///
/// # Errors
///
/// Refuses to touch either file when the settings file was written by a
/// newer schema version, when it cannot be read, or when any earlier
/// change is still unresolved.
pub fn save_affix_presets_with_selection(
    root: &Path,
    presets: &[AffixPreset],
    selected: Option<&str>,
) -> Result<CommittedRevision, ConfigError> {
    let mut settings = load_settings_for_write(root)?;
    keep_settings_migration_backup(root)?;
    ensure_existing_presets_readable(root)?;
    settings.selected_affix_preset = selected.unwrap_or_default().to_string();
    let settings_json = app_settings_json(&settings);
    let presets_json = affix_presets_json(presets);
    persistence::prepare_transaction(
        root,
        &[
            persistence::FileWrite {
                relative_path: AFFIX_PRESETS_PATH,
                contents: &presets_json,
            },
            persistence::FileWrite {
                relative_path: SETTINGS_PATH,
                contents: &settings_json,
            },
        ],
    )?
    .commit()
}

/// Parse exactly one `AffixPreset` from a JSON file's text — the Local
/// tab's import file format.
///
/// Upstream: `ImportAffixPresetCommandExecute`
/// (ImportAffixPresetViewModel.cs:1498-1538) deserializes a single
/// `AffixPreset` entity from the picked file (`JsonSerializer
/// .Deserialize<AffixPreset>`), so the file holds one preset object, not
/// the array `load_affix_presets` reads. The object shape, entry fields,
/// and `ParagonBoardsList` rules are the persisted format's — the same
/// parser [`Self`] fields follow.
pub fn parse_affix_preset_json(text: &str) -> Result<AffixPreset, String> {
    let document: Value = serde_json::from_str(text).map_err(|error| {
        format!("preset file must be one JSON object (an exported AffixPreset): {error}")
    })?;
    if !document.is_object() {
        return Err("preset file must be one JSON object (an exported AffixPreset)".to_string());
    }
    preset_from_json(&document, "preset file").map_err(String::from)
}

/// Serialize exactly one preset as the upstream-shaped indented JSON
/// object — the Local tab's export file format.
///
/// Upstream: `ExportAffixPresetCommandExecute`
/// (ImportAffixPresetViewModel.cs:882-894) serializes the selected
/// `AffixPreset` with `WriteIndented = true` into
/// `Exports/{SelectedAffixPreset.Name}.json`. The bytes match the
/// per-preset body of [`save_affix_presets`]'s array (plus the trailing
/// newline), so an exported file stays loadable by upstream's
/// `Deserialize<AffixPreset>` and re-importable by this build.
pub fn affix_preset_to_indented_json(preset: &AffixPreset) -> String {
    json::to_file_text(&affix_preset_object(preset))
}

/// Persist the last selected affix preset into `Config/Settings.json`.
///
/// Loads the current settings, replaces only the
/// [`AppSettings::selected_affix_preset`](crate::AppSettings::selected_affix_preset)
/// field, and saves the file — the same load-modify-save pattern as
/// [`save_wayland_restore_token`](crate::save_wayland_restore_token).
/// `None` stores the upstream default (empty string = "no selection").
/// The write goes through [`save_app_settings`](crate::save_app_settings),
/// which refuses to overwrite settings files written by a newer schema
/// version.
///
/// # Errors
///
/// Returns the load or write error; the caller surfaces it instead of
/// silently dropping the selection.
pub fn save_selected_affix_preset(
    root: &Path,
    name: Option<&str>,
) -> Result<CommittedRevision, ConfigError> {
    let mut settings = load_settings_for_write(root)?;
    keep_settings_migration_backup(root)?;
    settings.selected_affix_preset = name.unwrap_or_default().to_string();
    persistence::write_atomic(root, SETTINGS_PATH, &app_settings_json(&settings))
}

fn parse_affix_presets(text: &str) -> Result<Vec<AffixPreset>, ConfigError> {
    // Upstream writes the preset list as a top-level JSON array
    // (`JsonSerializer.Serialize(stream, AffixPresets)`); anything else
    // is not a shape this build accepts.
    let document = parse_json_document(text, AFFIX_PRESETS_PATH)?;
    let Value::Array(presets) = document else {
        return Err(ConfigError::malformed(
            AFFIX_PRESETS_PATH,
            "must be a JSON array of presets",
        ));
    };
    presets
        .iter()
        .map(|preset| preset_from_json(preset, AFFIX_PRESETS_PATH))
        .collect()
}

/// Parse one preset object.
///
/// Missing fields backfill the upstream entity defaults and unknown
/// properties are ignored — exactly what the upstream
/// `JsonSerializer.Deserialize<AffixPreset>` does. `ParagonBoardsList`
/// loads as the upstream entity defines it; a file without the field
/// (every pre-paragon d4lc preset) backfills the empty entity default.
fn preset_from_json(value: &Value, source: &str) -> Result<AffixPreset, ConfigError> {
    let Some(object) = value.as_object() else {
        return Err(ConfigError::malformed(
            source,
            "every preset must be a JSON object",
        ));
    };
    Ok(AffixPreset {
        name: object_string(object, "Name").unwrap_or_default(),
        item_affixes: preset_entry_list(object, "ItemAffixes", source)?,
        item_aspects: preset_entry_list(object, "ItemAspects", source)?,
        item_sigils: preset_entry_list(object, "ItemSigils", source)?,
        item_uniques: preset_entry_list(object, "ItemUniques", source)?,
        item_runes: preset_entry_list(object, "ItemRunes", source)?,
        paragon_boards_list: paragon_boards_list(object, source)?,
    })
}

/// Parse the upstream `ParagonBoardsList`
/// (`List<List<ParagonBoard>>`, `AffixPreset.cs:11`): an outer array
/// of journey steps, each an array of board objects.
fn paragon_boards_list(
    object: &Map<String, Value>,
    source: &str,
) -> Result<Vec<Vec<PresetParagonBoard>>, ConfigError> {
    match object.get("ParagonBoardsList") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(steps)) => steps
            .iter()
            .map(|step| match step {
                Value::Array(boards) => boards
                    .iter()
                    .map(|board| paragon_board_from_json(board, source))
                    .collect(),
                _ => Err(ConfigError::malformed(
                    source,
                    "ParagonBoardsList steps must be JSON arrays",
                )),
            })
            .collect(),
        Some(_) => Err(ConfigError::malformed(
            source,
            "ParagonBoardsList must be a JSON array",
        )),
    }
}

/// Parse one upstream `ParagonBoard` object. Missing fields fall back
/// to the entity defaults (empty strings, no active nodes, `Index` 0),
/// matching what .NET deserialization would produce.
fn paragon_board_from_json(value: &Value, source: &str) -> Result<PresetParagonBoard, ConfigError> {
    let Some(object) = value.as_object() else {
        return Err(ConfigError::malformed(
            source,
            "every paragon board must be a JSON object",
        ));
    };
    Ok(PresetParagonBoard {
        name: object_string(object, "Name").unwrap_or_default(),
        glyph: object_string(object, "Glyph").unwrap_or_default(),
        rotation: object_string(object, "Rotation").unwrap_or_default(),
        active_nodes: paragon_nodes(object.get("Nodes")),
        index: object
            .get("Index")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0),
    })
}

/// Parse the upstream `Nodes` field — the flattened 21×21 bool grid —
/// into d4lc's compact active-node positions (`x + y * 21`, in grid
/// order, which is ascending). A missing or non-array field yields no
/// active nodes; non-boolean tokens are skipped the same way .NET
/// would reject the whole file — d4lc keeps the loadable subset rather
/// than inventing positions.
fn paragon_nodes(value: Option<&Value>) -> Vec<usize> {
    value
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.as_bool() == Some(true))
                .map(|(position, _)| position)
                .collect()
        })
        .unwrap_or_default()
}

fn preset_entry_list(
    object: &Map<String, Value>,
    key: &str,
    source: &str,
) -> Result<Vec<PresetItemAffix>, ConfigError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| preset_item_affix_from_json(entry, source))
            .collect(),
        Some(_) => Err(ConfigError::malformed(
            source,
            format!("{key} must be a JSON array"),
        )),
    }
}

/// Parse one upstream `ItemAffix`. `IsAnyType` and `TuningPrisms` are
/// first-class typed fields (the preset model keeps them); a field this build does not know is ignored the way .NET's
/// default deserialization ignores it.
fn preset_item_affix_from_json(
    value: &Value,
    source: &str,
) -> Result<PresetItemAffix, ConfigError> {
    let Some(object) = value.as_object() else {
        return Err(ConfigError::malformed(
            source,
            "every preset entry must be a JSON object",
        ));
    };
    Ok(PresetItemAffix {
        id: object_string(object, "Id").unwrap_or_default(),
        item_type: object_string(object, "Type").unwrap_or_default(),
        color: preset_color_from_json(object.get("Color")),
        is_any_type: object_bool(object, "IsAnyType"),
        is_greater: object_bool(object, "IsGreater"),
        is_implicit: object_bool(object, "IsImplicit"),
        is_tempered: object_bool(object, "IsTempered"),
        tuning_prisms: object_string_list(object, "TuningPrisms"),
        label: object_string(object, "Label"),
    })
}

/// Parse the upstream `Color` field: the object System.Text.Json emits for
/// `System.Windows.Media.Color`, or (lenient, hand-edited files) d4lc's own
/// string form. Missing or partially valid colors fall back to the upstream
/// `Colors.Green` default instead of half-parsed bytes.
fn preset_color_from_json(value: Option<&Value>) -> PresetColor {
    value
        .and_then(crate::wpf_color::color_from_json)
        .unwrap_or(PresetColor::GREEN)
}

fn object_string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

fn object_bool(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The string entries of an upstream `List<string>` field; a missing or
/// non-array field is empty and non-string entries are skipped, so a
/// damaged list never discards the rest of the file.
fn object_string_list(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a complete JSON document, reporting the file it came from.
fn parse_json_document(text: &str, source: &str) -> Result<Value, ConfigError> {
    serde_json::from_str(text).map_err(|error| ConfigError::malformed(source, error.to_string()))
}

/// Serialize the affix presets as the upstream-shaped indented JSON
/// array (upstream `SaveAffixPresets` uses `WriteIndented = true`).
fn affix_presets_json(presets: &[AffixPreset]) -> String {
    json::to_file_text(&presets.iter().map(affix_preset_object).collect::<Vec<_>>())
}

/// One preset with the keys of `AffixPreset.cs`. `ParagonBoardsList` is
/// written in the upstream declaration order (last property) when the
/// preset carries boards, and omitted when empty — upstream backfills the
/// entity default on load.
fn affix_preset_object(preset: &AffixPreset) -> json::Object {
    let entries = |entries: &[PresetItemAffix]| {
        entries
            .iter()
            .map(preset_item_affix_object)
            .collect::<Vec<_>>()
    };
    // `SaveAffixPresets` sorts `ItemAffixes` before writing them.
    let mut item_affixes = preset.item_affixes.clone();
    sort_item_affixes(&mut item_affixes);
    let object = json::Object::new()
        .field("Name", &preset.name)
        .field("ItemAffixes", entries(&item_affixes))
        .field("ItemAspects", entries(&preset.item_aspects))
        .field("ItemSigils", entries(&preset.item_sigils))
        .field("ItemUniques", entries(&preset.item_uniques))
        .field("ItemRunes", entries(&preset.item_runes));
    if preset.paragon_boards_list.is_empty() {
        object
    } else {
        object.field(
            "ParagonBoardsList",
            preset
                .paragon_boards_list
                .iter()
                .map(|step| step.iter().map(paragon_board_object).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        )
    }
}

/// One upstream `ParagonBoard`. `Nodes` is expanded from d4lc's compact
/// active-node positions into the upstream flattened 21×21 bool grid
/// (`ParagonBoard.cs:14`), so the written file stays loadable by upstream.
fn paragon_board_object(board: &PresetParagonBoard) -> json::Object {
    let mut nodes = vec![false; PARAGON_GRID_LEN];
    for position in board.active_nodes.iter().copied() {
        if let Some(slot) = nodes.get_mut(position) {
            *slot = true;
        }
    }
    json::Object::new()
        .field("Name", &board.name)
        .field("Glyph", &board.glyph)
        .field("Rotation", &board.rotation)
        .field("Nodes", nodes)
        .field("Index", board.index)
}

/// One `ItemAffix` entry. The d4lc-local `Label` extension is written only
/// when set, so label-less entries keep the upstream shape.
fn preset_item_affix_object(entry: &PresetItemAffix) -> json::Object {
    let object = json::Object::new()
        .field("Id", &entry.id)
        .field("Type", &entry.item_type)
        .field("Color", crate::wpf_color::color_object(entry.color))
        .field("IsAnyType", entry.is_any_type)
        .field("IsGreater", entry.is_greater)
        .field("IsImplicit", entry.is_implicit)
        .field("IsTempered", entry.is_tempered)
        .field("TuningPrisms", &entry.tuning_prisms);
    match &entry.label {
        Some(label) => object.field("Label", label),
        None => object,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::{create_dir, create_file, create_temp_root, remove_dir};
    use crate::*;
    use std::fs;

    #[test]
    fn preset_color_try_parse_rejects_unknown_forms() {
        // The lenient parser is the stored-preset path; the strict
        // variant is what the ColorsConfig dialog validates against.
        assert_eq!(
            PresetColor::try_parse("#008000").expect("#008000 should parse"),
            PresetColor::GREEN
        );
        assert_eq!(
            PresetColor::try_parse("  green  ").expect("padded green should parse"),
            PresetColor::GREEN
        );
        assert!(PresetColor::try_parse("#00FF00AA00").is_err());
        assert!(PresetColor::try_parse("ultramarine").is_err());
        assert!(PresetColor::try_parse("").is_err());
        // The lenient variant still falls back to green.
        assert_eq!(PresetColor::parse("ultramarine"), PresetColor::GREEN);
    }

    #[test]
    fn missing_affix_presets_file_yields_empty_state() {
        let root = create_temp_root("missing_affix_presets_file_yields_empty_state");
        let presets =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert!(presets.is_empty(), "a fresh install starts with no presets");
        remove_dir(&root);
    }

    #[test]
    fn loads_upstream_shaped_affix_presets_with_all_entry_fields() {
        let root = create_temp_root("loads_upstream_shaped_affix_presets");
        // Fixture in the upstream shape: every `ItemAffix` field is
        // present exactly as upstream writes it, including the
        // `IsAnyType` flag and the `TuningPrisms` list, so the complete
        // entity survives the load instead of being projected away. The
        // ParagonBoardsList is present with a populated journey step
        // exactly as upstream serializes it (flattened 21×21 bool Nodes
        // grid) and must load into the d4lc model.
        create_file(
            &root.join(AFFIX_PRESETS_PATH),
            r##"[
  {
    "Name": "Boss",
    "ItemAffixes": [
      { "Id": "AttackSpeed;+1", "Type": "Amulet", "Color": { "A": 255, "R": 34, "G": 197, "B": 94 }, "IsAnyType": false, "IsGreater": true, "IsImplicit": false, "IsTempered": true, "TuningPrisms": [ "Prism1", "Prism2" ] },
      { "Id": "Life", "Type": "Boots", "Color": {}, "IsAnyType": true, "IsGreater": false, "IsImplicit": true, "IsTempered": false, "TuningPrisms": [] }
    ],
    "ItemAspects": [
      { "Id": "RapidAspect", "Type": "Gloves", "Color": { "A": 255, "R": 0, "G": 255, "B": 255 }, "IsAnyType": false, "IsGreater": false, "IsImplicit": false, "IsTempered": false, "TuningPrisms": [] }
    ],
    "ItemSigils": [],
    "ItemUniques": [
      { "Id": "Frostburn", "Type": "", "Color": { "A": 255, "R": 255, "G": 165, "B": 0 }, "IsAnyType": false, "IsGreater": false, "IsImplicit": false, "IsTempered": false, "TuningPrisms": [] }
    ],
    "ItemRunes": [],
    "ParagonBoardsList": [
      [ { "Name": "Start", "Glyph": "Enchanter", "Rotation": "90°", "Nodes": [false, true, false], "Index": 0 } ],
      []
    ]
  }
]"##,
        );

        let presets =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(presets.len(), 1);
        let preset = &presets[0];
        assert_eq!(preset.name, "Boss");
        assert_eq!(preset.item_affixes.len(), 2);
        let tempered = &preset.item_affixes[0];
        assert_eq!(tempered.id, "AttackSpeed;+1");
        assert_eq!(tempered.item_type, "Amulet");
        assert_eq!(tempered.color, PresetColor::from_rgb(34, 197, 94));
        assert!(!tempered.is_any_type);
        assert!(tempered.is_greater);
        assert!(!tempered.is_implicit);
        assert!(tempered.is_tempered);
        assert_eq!(tempered.tuning_prisms, vec!["Prism1", "Prism2"]);
        // An "any item type" entry keeps BOTH the imported item type and
        // the independent flag (the flag is not derived from the type); an
        // empty upstream Color object falls back to the
        // documented `Colors.Green` default.
        let any_type = &preset.item_affixes[1];
        assert_eq!(any_type.color, PresetColor::GREEN);
        assert_eq!(any_type.item_type, "Boots");
        assert!(any_type.is_any_type);
        assert!(any_type.tuning_prisms.is_empty());
        assert_eq!(preset.item_aspects.len(), 1);
        assert_eq!(preset.item_uniques.len(), 1);
        assert!(preset.item_sigils.is_empty());
        assert!(preset.item_runes.is_empty());
        // The populated upstream journey step loads as one board with
        // the active grid position; the empty second step is kept so
        // step positions survive a round trip.
        assert_eq!(preset.paragon_boards_list.len(), 2);
        let board = &preset.paragon_boards_list[0][0];
        assert_eq!(board.name, "Start");
        assert_eq!(board.glyph, "Enchanter");
        assert_eq!(board.rotation, "90°");
        assert_eq!(board.active_nodes, vec![1]);
        assert_eq!(board.index, 0);
        assert!(preset.paragon_boards_list[1].is_empty());

        remove_dir(&root);
    }

    #[test]
    fn preset_entries_carry_any_type_and_prisms_through_save_and_load() {
        // A preset written by d4lc keeps every entry field,
        // including the upstream-shape `IsAnyType`/`TuningPrisms`, and
        // the uploaded bytes are the ones a reload sees.
        let root = create_temp_root("preset_entry_fields_round_trip");
        let presets = vec![AffixPreset {
            name: "Boss".to_string(),
            item_affixes: vec![PresetItemAffix {
                id: "AttackSpeed;+1".to_string(),
                item_type: "Boots".to_string(),
                color: PresetColor::from_rgb(34, 197, 94),
                is_any_type: true,
                is_greater: false,
                is_implicit: false,
                is_tempered: true,
                tuning_prisms: vec!["Prism of Control".to_string(), "Prism2".to_string()],
                label: Some("Attack Speed".to_string()),
            }],
            item_aspects: Vec::new(),
            item_sigils: Vec::new(),
            item_uniques: Vec::new(),
            item_runes: Vec::new(),
            paragon_boards_list: Vec::new(),
        }];

        let revision = save_affix_presets(&root, &presets)
            .unwrap_or_else(|error| panic!("save failed: {error}"));
        assert_eq!(revision.files, vec![AFFIX_PRESETS_PATH.to_string()]);
        let text = fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read persisted");
        let document: Value = serde_json::from_str(&text).expect("presets file is JSON");
        let entry = &document[0]["ItemAffixes"][0];
        assert_eq!(
            entry["IsAnyType"], true,
            "the any-type flag must be written"
        );
        assert_eq!(
            entry["TuningPrisms"],
            serde_json::json!(["Prism of Control", "Prism2"]),
            "the prism list must be written in order"
        );

        let reloaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(reloaded, presets);
        let entry = &reloaded[0].item_affixes[0];
        assert!(entry.is_any_type && entry.item_type == "Boots");
        assert_eq!(entry.tuning_prisms.len(), 2);

        // The persisted bytes are the revision the caller may report:
        // re-saving the same value yields the same digest.
        let again = save_affix_presets(&root, &presets)
            .unwrap_or_else(|error| panic!("save failed: {error}"));
        assert_eq!(revision, again);

        remove_dir(&root);
    }

    #[test]
    fn unicode_escapes_and_surrogate_pairs_round_trip_losslessly() {
        // Valid `\uXXXX` escapes and surrogate pairs (an astral
        // emoji) decode to their text and reload unchanged; the old
        // hand-rolled decoder passed unknown escapes through and broke
        // pairs.
        let root = create_temp_root("unicode_escapes_round_trip");
        create_file(
            &root.join(AFFIX_PRESETS_PATH),
            r##"[
  {
    "Name": "Boss \u00fcn\u00df\uD83D\uDE00",
    "ItemAffixes": [
      { "Id": "Id\u00e4\uD83D\uDE00", "Type": "Amulet\u00e9", "Color": { "A": 255, "R": 1, "G": 2, "B": 3 }, "IsAnyType": true, "IsGreater": false, "IsImplicit": false, "IsTempered": false, "TuningPrisms": [ "Prism\uD83D\uDE00\u00fc" ] }
    ]
  }
]"##,
        );

        let presets =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(presets[0].name, "Boss ünß😀");
        assert_eq!(presets[0].item_affixes[0].id, "Idä😀");
        assert_eq!(presets[0].item_affixes[0].item_type, "Amuleté");
        assert_eq!(
            presets[0].item_affixes[0].tuning_prisms,
            vec!["Prism😀ü".to_string()]
        );

        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("save failed: {error}"));
        let reloaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(reloaded, presets, "the escape-decoded text must survive");
        let text = fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read persisted");
        assert!(
            text.contains("Boss ünß😀"),
            "the writer emits the text itself (UTF-8), not broken escapes: {text}"
        );

        remove_dir(&root);
    }

    #[test]
    fn preset_selection_change_commits_both_files_as_one_revision() {
        let root = create_temp_root("preset_selection_transaction");
        let presets = vec![AffixPreset {
            name: "Speed".to_string(),
            ..AffixPreset::default()
        }];
        let settings = AppSettings::default();
        save_app_settings(&root, &settings)
            .unwrap_or_else(|error| panic!("seed settings: {error}"));

        let revision =
            save_affix_presets_with_selection(&root, &presets, Some("Speed")).expect("commit");
        assert_eq!(
            revision.files,
            vec![AFFIX_PRESETS_PATH.to_string(), SETTINGS_PATH.to_string()]
        );
        assert_eq!(
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load presets: {error}")),
            presets
        );
        assert_eq!(
            load_app_settings(&root)
                .unwrap_or_else(|error| panic!("load settings: {error}"))
                .selected_affix_preset,
            "Speed"
        );
        assert!(
            interrupted_transactions(&root).expect("list").is_empty(),
            "a finished change leaves no record behind"
        );

        remove_dir(&root);
    }

    #[test]
    fn preset_selection_change_refuses_a_newer_settings_schema_without_touching_files() {
        // / unknown newer schema: a settings file from a future
        // build is never partially overwritten; the preset file and the
        // settings file stay byte-identical and no revision is reported.
        let root = create_temp_root("preset_selection_newer_schema");
        let presets = vec![AffixPreset {
            name: "Speed".to_string(),
            ..AffixPreset::default()
        }];
        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("seed presets: {error}"));
        let future = r##"{ "schema_version": 3, "SelectedSystemPreset": "1080p_SMF" }"##;
        create_file(&root.join(SETTINGS_PATH), future);
        let presets_before =
            fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read presets");

        let error = save_affix_presets_with_selection(&root, &presets, Some("Boss"))
            .expect_err("a newer settings schema must be refused");
        assert!(
            matches!(error, ConfigError::NewerSchema { version: 3, .. }),
            "unexpected: {error}"
        );
        assert_eq!(
            fs::read_to_string(root.join(SETTINGS_PATH)).expect("read settings"),
            future,
            "the future file must survive unchanged"
        );
        assert_eq!(
            fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read presets"),
            presets_before,
            "the preset file must not be replaced when the change is refused"
        );
        assert!(interrupted_transactions(&root).expect("list").is_empty());

        remove_dir(&root);
    }

    #[test]
    fn preset_selection_change_failure_between_replacements_is_detected() {
        // A write failure between the two replacements must not
        // report success and must not look like a legitimate state; the
        // record names the partial change and rollback restores the
        // previous preset file.
        let root = create_temp_root("preset_selection_interrupted");
        let old_presets = vec![AffixPreset {
            name: "Old".to_string(),
            ..AffixPreset::default()
        }];
        let new_presets = vec![AffixPreset {
            name: "New".to_string(),
            ..AffixPreset::default()
        }];
        save_affix_presets(&root, &old_presets).expect("seed presets");
        let settings = AppSettings::default();
        save_app_settings(&root, &settings).expect("seed settings");
        // The settings target becomes a directory after the change is
        // prepared, so the preset replacement succeeds and the settings
        // replacement fails.
        let settings_target = root.join(SETTINGS_PATH);
        fs::remove_file(&settings_target).expect("remove settings");
        create_dir(&settings_target);

        let error = save_affix_presets_with_selection(&root, &new_presets, Some("New"))
            .expect_err("the settings replacement must fail");
        assert!(
            matches!(error, ConfigError::CommitInterrupted { .. }),
            "unexpected: {error}"
        );

        let records = interrupted_transactions(&root).expect("detect");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].files(),
            vec![AFFIX_PRESETS_PATH, SETTINGS_PATH],
            "the record names exactly the changed files"
        );
        records
            .into_iter()
            .next()
            .expect("one record")
            .recover(Recovery::Rollback)
            .expect("rollback");
        assert_eq!(
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load presets: {error}")),
            old_presets,
            "rollback restores the preset file the interrupted change had replaced"
        );
        assert!(interrupted_transactions(&root).expect("list").is_empty());

        remove_dir(&root);
    }

    fn test_entry(id: &str, is_implicit: bool, is_tempered: bool) -> PresetItemAffix {
        PresetItemAffix {
            id: id.to_string(),
            item_type: String::new(),
            color: PresetColor::GREEN,
            is_any_type: false,
            is_greater: false,
            is_implicit,
            is_tempered,
            tuning_prisms: Vec::new(),
            label: None,
        }
    }

    fn test_preset(name: &str, item_affixes: Vec<PresetItemAffix>) -> AffixPreset {
        AffixPreset {
            name: name.to_string(),
            item_affixes,
            item_aspects: Vec::new(),
            item_sigils: Vec::new(),
            item_uniques: Vec::new(),
            item_runes: Vec::new(),
            paragon_boards_list: Vec::new(),
        }
    }

    #[test]
    fn loaded_presets_are_sorted_by_ordinal_name() {
        let root = create_temp_root("loaded_presets_are_sorted_by_ordinal_name");
        let presets = vec![
            test_preset("beta", Vec::new()),
            test_preset("Zulu", Vec::new()),
            test_preset("alpha", Vec::new()),
            test_preset("Alpha", Vec::new()),
        ];
        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("save failed: {error}"));
        let names: Vec<String> = load_affix_presets(&root)
            .unwrap_or_else(|error| panic!("load failed: {error}"))
            .into_iter()
            .map(|preset| preset.name)
            .collect();
        // Ordinal: every uppercase letter sorts before every lowercase one.
        assert_eq!(names, ["Alpha", "Zulu", "alpha", "beta"]);
        remove_dir(&root);
    }

    #[test]
    fn ordinal_compare_uses_utf16_code_units() {
        use std::cmp::Ordering;
        // U+1F600 is a surrogate pair (0xD83D..) in UTF-16, which sorts before
        // U+FF21; in UTF-8 byte order it sorts after.
        assert_eq!(compare_ordinal("\u{1F600}", "\u{FF21}"), Ordering::Less);
        assert_eq!(compare_ordinal("a", "B"), Ordering::Greater);
        assert_eq!(compare_ordinal("same", "same"), Ordering::Equal);
    }

    #[test]
    fn saved_item_affixes_follow_the_upstream_sort() {
        let root = create_temp_root("saved_item_affixes_follow_the_upstream_sort");
        let preset = test_preset(
            "Sorted",
            vec![
                test_entry("Zed", false, true),
                test_entry("Beta", false, false),
                test_entry("Alpha", true, true),
                test_entry("Zeta", true, false),
                test_entry("alpha", false, false),
                test_entry("Alpha", false, false),
                test_entry("Alpha_Two", false, false),
                test_entry("Alpha2", false, false),
            ],
        );
        save_affix_presets(&root, &[preset]).unwrap_or_else(|error| panic!("save failed: {error}"));
        let loaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        let order: Vec<(&str, bool, bool)> = loaded[0]
            .item_affixes
            .iter()
            .map(|entry| (entry.id.as_str(), entry.is_implicit, entry.is_tempered))
            .collect();
        // Not tempered first, implicit before the rest, then by id (culture
        // order: punctuation, digits, letters; lowercase before uppercase).
        assert_eq!(
            order,
            [
                ("Zeta", true, false),
                ("alpha", false, false),
                ("Alpha", false, false),
                ("Alpha_Two", false, false),
                ("Alpha2", false, false),
                ("Beta", false, false),
                ("Alpha", true, true),
                ("Zed", false, true),
            ]
        );
        remove_dir(&root);
    }

    #[test]
    fn culture_compare_orders_ids_like_dotnet() {
        use std::cmp::Ordering;
        assert_eq!(compare_culture("a", "B"), Ordering::Less);
        assert_eq!(compare_culture("a", "A"), Ordering::Less);
        assert_eq!(compare_culture("A", "b"), Ordering::Less);
        assert_eq!(compare_culture("Life;1", "Life_1"), Ordering::Greater);
        assert_eq!(compare_culture("Life_1", "Life1"), Ordering::Less);
        assert_eq!(compare_culture("Life1", "LifeA"), Ordering::Less);
        assert_eq!(compare_culture("Life", "Life;x"), Ordering::Less);
        assert_eq!(compare_culture("Life", "Life"), Ordering::Equal);
    }

    #[test]
    fn affix_presets_round_trip_through_save_and_load() {
        let root = create_temp_root("affix_presets_round_trip_through_save_and_load");
        let presets = vec![
            AffixPreset {
                name: "Speed".to_string(),
                item_affixes: vec![PresetItemAffix {
                    id: "AttackSpeed;+1".to_string(),
                    item_type: String::new(),
                    color: PresetColor::from_rgb(0, 255, 0),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: true,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                    label: None,
                }],
                item_aspects: Vec::new(),
                item_sigils: Vec::new(),
                item_uniques: Vec::new(),
                item_runes: Vec::new(),
                paragon_boards_list: Vec::new(),
            },
            AffixPreset {
                name: "Boss".to_string(),
                item_affixes: Vec::new(),
                item_aspects: Vec::new(),
                item_sigils: vec![PresetItemAffix {
                    id: "SigilOfStuff".to_string(),
                    item_type: "Sigil".to_string(),
                    color: PresetColor::parse("#FFAA00"),
                    is_any_type: false,
                    is_greater: false,
                    is_implicit: false,
                    is_tempered: false,
                    tuning_prisms: Vec::new(),
                    label: None,
                }],
                item_uniques: Vec::new(),
                item_runes: Vec::new(),
                paragon_boards_list: Vec::new(),
            },
        ];

        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("save failed: {error}"));
        let reloaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        // The loaded list is sorted by name (AffixManager.cs:920-924).
        assert_eq!(reloaded, vec![presets[1].clone(), presets[0].clone()]);

        // Upstream-shape spot checks on the written bytes: upstream
        // entity keys, the Media.Color byte object, and the top-level
        // array (so upstream's `Deserialize<List<AffixPreset>>` reads
        // the file back).
        let text = fs::read_to_string(root.join(AFFIX_PRESETS_PATH))
            .unwrap_or_else(|error| panic!("failed to read presets: {error}"));
        let document: Value = serde_json::from_str(&text).expect("presets file is JSON");
        let preset = &document[0];
        for key in [
            "Name",
            "ItemAffixes",
            "ItemAspects",
            "ItemSigils",
            "ItemUniques",
            "ItemRunes",
        ] {
            assert!(preset.get(key).is_some(), "saved preset must carry {key}");
        }
        let entry = &preset["ItemAffixes"][0];
        for key in ["Id", "Type", "IsGreater", "IsImplicit", "IsTempered"] {
            assert!(entry.get(key).is_some(), "saved entry must carry {key}");
        }
        let orange = serde_json::json!({
            "ColorContext": null,
            "A": 255,
            "R": 255,
            "G": 170,
            "B": 0,
            "ScA": 1,
            "ScR": 1,
            "ScG": 0.4019778,
            "ScB": 0
        });
        let families = [
            "ItemAffixes",
            "ItemAspects",
            "ItemSigils",
            "ItemUniques",
            "ItemRunes",
        ];
        let colors: Vec<&Value> = document
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|preset| {
                families
                    .iter()
                    .filter_map(|family| preset[*family].as_array())
            })
            .flatten()
            .map(|entry| &entry["Color"])
            .collect();
        assert!(
            colors.contains(&&orange),
            "a color is written as the Media.Color object: {colors:?}"
        );
        // Paragon-less presets omit the field instead of writing it empty.
        assert!(
            preset.get("ParagonBoardsList").is_none(),
            "an empty paragon list must not be written"
        );
        // A save into a root without Config/ creates the directory,
        // mirroring upstream's Directory.CreateDirectory.
        assert!(root.join("Config").is_dir());

        remove_dir(&root);
    }

    #[test]
    fn single_preset_import_export_round_trip() {
        // The Local tab's file format (upstream
        // `ExportAffixPresetCommandExecute` / `ImportAffixPresetCommand
        // Execute`): one preset object serialized indented, parsed back
        // to the same value. Export output starts with the object brace
        // (NOT the array of the preset file), and the parse rejects
        // non-object shapes like upstream's Deserialize<AffixPreset>.
        let preset = AffixPreset {
            name: "Speed".to_string(),
            item_affixes: vec![PresetItemAffix {
                id: "AttackSpeed;+1".to_string(),
                item_type: String::new(),
                color: PresetColor::from_rgb(34, 197, 94),
                is_any_type: false,
                is_greater: true,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
                label: Some("Attack Speed".to_string()),
            }],
            item_aspects: Vec::new(),
            item_sigils: Vec::new(),
            item_uniques: Vec::new(),
            item_runes: Vec::new(),
            paragon_boards_list: Vec::new(),
        };
        let text = affix_preset_to_indented_json(&preset);
        assert!(
            text.trim_start().starts_with('{'),
            "the export format is one object, not an array"
        );
        assert!(text.contains("\"Name\": \"Speed\""));
        let parsed =
            parse_affix_preset_json(&text).unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(parsed, preset);

        // The array shape of the preset file is not a single preset —
        // the Local tab import reports it as a failure like upstream's
        // deserialization error.
        assert!(parse_affix_preset_json("[]").is_err());
        assert!(parse_affix_preset_json("{ broken").is_err());
    }

    #[test]
    fn preset_entry_label_persists_and_label_less_files_keep_their_shape() {
        // The d4lc-extension `Label` field survives a save/load round
        // trip (marker labels survive restarts), files written before
        // the field existed load unchanged with `label: None`, and a
        // label-less entry still writes the historical upstream-shaped
        // bytes (no trailing `Label` key).
        let root = create_temp_root("preset_entry_label_persists");
        let presets = vec![AffixPreset {
            name: "Speed".to_string(),
            item_affixes: vec![PresetItemAffix {
                id: "AttackSpeed;+1".to_string(),
                item_type: "Amulet".to_string(),
                color: PresetColor::from_rgb(34, 197, 94),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
                label: Some("Attack Speed".to_string()),
            }],
            item_aspects: Vec::new(),
            item_sigils: Vec::new(),
            item_uniques: Vec::new(),
            item_runes: Vec::new(),
            paragon_boards_list: Vec::new(),
        }];

        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("save failed: {error}"));
        let text =
            fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read persisted presets");
        assert!(
            text.contains("\"Label\": \"Attack Speed\""),
            "the d4lc Label extension must be written: {text}"
        );
        let reloaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(reloaded, presets);
        assert_eq!(
            reloaded[0].item_affixes[0].label.as_deref(),
            Some("Attack Speed")
        );

        // A pre-label d4lc/upstream file without the field: tolerant
        // load backfills `None`, and re-saving restores the historical
        // bytes (no `Label` key at all).
        let legacy = "[ { \"Name\": \"Old\", \"ItemAffixes\": [ { \"Id\": \"Life\", \"Type\": \"\", \
                      \"Color\": { \"A\": 255, \"R\": 0, \"G\": 128, \"B\": 0 }, \"IsGreater\": false, \
                      \"IsImplicit\": false, \"IsTempered\": false } ] } ]";
        fs::write(root.join(AFFIX_PRESETS_PATH), legacy).expect("write legacy fixture");
        let loaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(loaded[0].item_affixes[0].label, None);
        save_affix_presets(&root, &loaded).unwrap_or_else(|error| panic!("save failed: {error}"));
        let rewritten =
            fs::read_to_string(root.join(AFFIX_PRESETS_PATH)).expect("read rewritten presets");
        assert!(
            !rewritten.contains("Label"),
            "a label-less entry keeps the historical file shape: {rewritten}"
        );

        remove_dir(&root);
    }

    #[test]
    fn affix_preset_paragon_boards_round_trip_through_save_and_load() {
        let root = create_temp_root("affix_preset_paragon_boards_round_trip");
        // Two journey steps exactly as an import produces them: the
        // starter board plus one 90°-rotated board with localized
        // labels and transformed node positions.
        let presets = vec![AffixPreset {
            name: "Whirlwind".to_string(),
            item_affixes: Vec::new(),
            item_aspects: Vec::new(),
            item_sigils: Vec::new(),
            item_uniques: Vec::new(),
            item_runes: Vec::new(),
            paragon_boards_list: vec![
                vec![PresetParagonBoard {
                    name: "Start".to_string(),
                    glyph: String::new(),
                    rotation: "0°".to_string(),
                    active_nodes: vec![105],
                    index: 0,
                }],
                vec![
                    PresetParagonBoard {
                        name: "Hemorrhage".to_string(),
                        glyph: "Enchanter".to_string(),
                        rotation: "90°".to_string(),
                        active_nodes: vec![36, 57],
                        index: 1,
                    },
                    PresetParagonBoard {
                        name: "Flourish".to_string(),
                        glyph: "Deadly".to_string(),
                        rotation: "270°".to_string(),
                        active_nodes: Vec::new(),
                        index: 2,
                    },
                ],
            ],
        }];

        save_affix_presets(&root, &presets).unwrap_or_else(|error| panic!("save failed: {error}"));
        let reloaded =
            load_affix_presets(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(
            reloaded, presets,
            "the paragon list must survive a save/load cycle"
        );

        // Upstream-shape spot checks on the written bytes: the
        // ParagonBoardsList key in upstream declaration order (last
        // property), the per-board entity keys, and the flattened
        // 21×21 bool grid with the active positions set.
        let text = fs::read_to_string(root.join(AFFIX_PRESETS_PATH))
            .unwrap_or_else(|error| panic!("failed to read presets: {error}"));
        let document: Value = serde_json::from_str(&text).expect("presets file is JSON");
        let steps = document[0]["ParagonBoardsList"]
            .as_array()
            .expect("ParagonBoardsList is written");
        let boards: Vec<&Value> = steps
            .iter()
            .flat_map(|step| step.as_array().expect("a step is an array"))
            .collect();
        assert_eq!(boards.len(), 3, "every board must be written");
        assert_eq!(boards[0]["Name"], "Start");
        assert_eq!(boards[1]["Glyph"], "Enchanter");
        assert_eq!(boards[1]["Rotation"], "90°");
        assert_eq!(boards[1]["Index"], 1);
        let true_positions: Vec<usize> = boards
            .iter()
            .flat_map(|board| {
                let nodes = board["Nodes"].as_array().expect("Nodes is an array");
                assert_eq!(
                    nodes.len(),
                    PARAGON_GRID_LEN,
                    "every Nodes array must carry the full 21×21 grid"
                );
                nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| node.as_bool() == Some(true))
                    .map(|(position, _)| position)
                    .collect::<Vec<_>>()
            })
            .collect();
        // Board order in the file: Start (105 active), Hemorrhage
        // (36 + 57), Flourish (none).
        assert_eq!(true_positions, vec![105, 36, 57]);

        remove_dir(&root);
    }

    #[test]
    fn unreadable_affix_presets_file_is_an_error_not_an_empty_state() {
        let root = create_temp_root("unreadable_affix_presets_file_is_an_error");
        create_file(&root.join(AFFIX_PRESETS_PATH), "{ not an array");

        assert!(
            load_affix_presets(&root).is_err(),
            "a malformed file must surface as an error, never silently load as empty"
        );

        remove_dir(&root);
    }

    #[test]
    fn preset_color_parses_string_forms_and_falls_back_to_green() {
        assert_eq!(
            PresetColor::parse("#00FF00"),
            PresetColor::from_rgb(0, 255, 0)
        );
        assert_eq!(PresetColor::parse("#0F0"), PresetColor::from_rgb(0, 255, 0));
        assert_eq!(
            PresetColor::parse("#80FFAA00"),
            PresetColor {
                a: 128,
                r: 255,
                g: 170,
                b: 0
            }
        );
        assert_eq!(
            PresetColor::parse("cyan"),
            PresetColor::from_rgb(0, 255, 255)
        );
        assert_eq!(PresetColor::parse("#ZZZZZZ"), PresetColor::GREEN);
        assert_eq!(PresetColor::parse(""), PresetColor::GREEN);
        assert_eq!(
            PresetColor::from_rgb(34, 197, 94).to_string_form(),
            "#22C55E"
        );
        assert_eq!(
            PresetColor {
                a: 128,
                r: 255,
                g: 170,
                b: 0
            }
            .to_string_form(),
            "#80FFAA00"
        );
    }

    #[test]
    fn save_selected_affix_preset_updates_only_that_field() {
        let root = create_temp_root("save_selected_affix_preset_updates_only_that_field");
        let settings = AppSettings {
            selected_system_preset: "1080p_SMF".to_string(),
            selected_affix_preset: "Speed".to_string(),
            ..AppSettings::default()
        };
        save_app_settings(&root, &settings).unwrap_or_else(|error| panic!("save failed: {error}"));

        save_selected_affix_preset(&root, Some("Boss"))
            .unwrap_or_else(|error| panic!("save failed: {error}"));
        let reloaded =
            load_app_settings(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(reloaded.selected_affix_preset, "Boss");
        assert_eq!(reloaded.selected_system_preset, "1080p_SMF");

        // An empty selection stores the upstream default (empty string).
        save_selected_affix_preset(&root, None)
            .unwrap_or_else(|error| panic!("save failed: {error}"));
        let cleared =
            load_app_settings(&root).unwrap_or_else(|error| panic!("load failed: {error}"));
        assert_eq!(cleared.selected_affix_preset, "");

        remove_dir(&root);
    }

    // -------------------------------------------------------------------
    // Trade-item persistence (Config/TradeItems.json)
    // -------------------------------------------------------------------
}
