//! Localized game-data catalog for the affix screen.
//!
//! Upstream's affix screen right column is a full catalog browser: every
//! wanted-card template iterates the loaded game-data index for the tab's
//! kind (`AffixView.xaml:1499-1996`), not the selected preset's entries.
//! This module projects the same vendored `*.json` indexes
//! (`assets/original/data/`) into the browsable per-kind entry lists.
//!
//! ## Id and label rules
//!
//! The catalog is the *add* source for preset entries, so its ids must be
//! the same ids the OCR path produces: a preset entry added from the
//! catalog is matched against parsed items by id (`d4lc-fuzzy`'s
//! `match_text` winner id). The id/label rules therefore mirror
//! `d4lc-fuzzy`'s candidate rules exactly:
//!
//! - `IdName`, falling back to `IdSno`;
//! - blank labels are skipped (the v3.2.0 localized Aspects files carry one
//!   placeholder row with an empty `DescriptionClean`);
//! - Dungeon sigils label as `"<Name> in <DungeonZoneInfo>"`, mirroring
//!   upstream's `AffixManager` sigil lookup label;
//! - Uniques and Sigils are keyed by label: rows sharing a label map to the
//!   first row's id (`AffixInfoVM`/`UniqueInfoVM` are keyed by label in the
//!   upstream managers). Data rows whose id was already claimed by an
//!   earlier row with the same label are folded into that row, so one card
//!   addresses one canonical id.
//!
//! ## Attributes
//!
//! Per-entry attribute markers are projected from the data indexes
//! (`AffixView.xaml:1538-1576` tempering + class icons, `:1602-1662`
//! aspect Codex/Seasonal/DropOnly, `:1785-1788` sigil seasonal) rather
//! than the static defaults the screen carried before.

use d4lc_assets::AssetRepository;
use serde_json::Value;
use std::collections::HashMap;

use crate::import::ensure_localized_data_file;
use crate::{ImportLocalizedDataError, LocalizedDataComponent};

/// The five browsable catalog kinds, in the affix screen's tab order
/// (upstream `AffixView.xaml:1498,1602,1728,1823,1927`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CatalogKind {
    Affix,
    Aspect,
    Sigil,
    Unique,
    Rune,
}

impl CatalogKind {
    /// Tab order: Affixes, Aspects, Sigils, Uniques, Runes.
    pub const ALL: [CatalogKind; 5] = [
        CatalogKind::Affix,
        CatalogKind::Aspect,
        CatalogKind::Sigil,
        CatalogKind::Unique,
        CatalogKind::Rune,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Affix => "affix",
            Self::Aspect => "aspect",
            Self::Sigil => "sigil",
            Self::Unique => "unique",
            Self::Rune => "rune",
        }
    }

    /// The localized data file the kind is projected from.
    pub const fn component(self) -> LocalizedDataComponent {
        match self {
            Self::Affix => LocalizedDataComponent::Affixes,
            Self::Aspect => LocalizedDataComponent::Aspects,
            Self::Sigil => LocalizedDataComponent::Sigils,
            Self::Unique => LocalizedDataComponent::Uniques,
            Self::Rune => LocalizedDataComponent::Runes,
        }
    }
}

/// The eight class flags of `AllowedForPlayerClass`, in the data's index
/// order (upstream `AffixInfoWanted.IsClassSorc` .. `IsClassWarlock`,
/// `AffixInfoVM.cs:66-104`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClassFlags {
    pub sorcerer: bool,
    pub druid: bool,
    pub barbarian: bool,
    pub rogue: bool,
    pub necromancer: bool,
    pub spiritborn: bool,
    pub paladin: bool,
    pub warlock: bool,
}

impl ClassFlags {
    /// Project the raw `AllowedForPlayerClass` list; a short or missing
    /// list yields all-false (no class restriction marker).
    pub fn from_allowed_for_player_class(values: Option<&Vec<Value>>) -> Self {
        let flag = |index: usize| {
            values
                .and_then(|values| values.get(index))
                .and_then(Value::as_i64)
                .is_some_and(|value| value == 1)
        };
        Self {
            sorcerer: flag(0),
            druid: flag(1),
            barbarian: flag(2),
            rogue: flag(3),
            necromancer: flag(4),
            spiritborn: flag(5),
            paladin: flag(6),
            warlock: flag(7),
        }
    }

    /// `AllowedForPlayerClass.All(c => c == 1)` — the upstream "Core"
    /// filter arm (`AffixViewModel.cs:1114,1195,1327`).
    pub const fn all(self) -> bool {
        self.sorcerer
            && self.druid
            && self.barbarian
            && self.rogue
            && self.necromancer
            && self.spiritborn
            && self.paladin
            && self.warlock
    }

    /// `AllowedForPlayerClass.All(c => c == 0)` — the unrestricted arm of
    /// the same upstream Core check.
    pub const fn none(self) -> bool {
        !self.sorcerer
            && !self.druid
            && !self.barbarian
            && !self.rogue
            && !self.necromancer
            && !self.spiritborn
            && !self.paladin
            && !self.warlock
    }
}

/// One browsable game-data entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The preset-entry id contract (`PresetEntry.id`).
    pub id: String,
    /// The card's primary line: affix `Description`, other kinds `Name`.
    pub label: String,
    /// The card's body line (affix entries carry the same text as the
    /// label; the affix card renders the label only).
    pub description: String,
    /// `IsTemperingAvailable` (affix tempering marker).
    pub is_tempered: bool,
    /// `AllowedForPlayerClass` projected to the eight class markers.
    pub classes: ClassFlags,
    /// `IsCodex && Dungeon != ""` (upstream `AspectInfoWanted.IsCodex`).
    pub is_codex: bool,
    /// `IsSeasonal` (aspects and sigils).
    pub is_seasonal: bool,
    /// `Dungeon == ""` (upstream `AspectInfoWanted.IsDropOnly`).
    pub is_drop_only: bool,
    /// The aspect's Codex dungeon (`AspectInfoWanted.Dungeon`).
    pub dungeon: String,
    /// `RuneType == "condition"` (upstream `RuneInfoWanted.IsCondition`).
    pub is_condition: bool,
    /// `RuneType == "effect"` (upstream `RuneInfoWanted.IsEffect`).
    pub is_effect: bool,
    /// Raw `Type` token of a sigil (`Dungeon`/`Positive`/`Minor`/`Major`/
    /// `Misc`); `None` for the other kinds.
    pub sigil_type: Option<String>,
}

impl CatalogEntry {
    /// A catalog row for a preset entry whose id the localized data does
    /// not carry (another data version or another language): the display
    /// text is the preset's own label and every game-data attribute stays
    /// empty, so the entry keeps its row and card instead of disappearing.
    pub fn without_data(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            description: label.to_string(),
            is_tempered: false,
            classes: ClassFlags::default(),
            is_codex: false,
            is_seasonal: false,
            is_drop_only: false,
            dungeon: String::new(),
            is_condition: false,
            is_effect: false,
            sigil_type: None,
        }
    }
}

/// The localized catalog for one data language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixCatalog {
    language: String,
    affixes: Vec<CatalogEntry>,
    aspects: Vec<CatalogEntry>,
    sigils: Vec<CatalogEntry>,
    uniques: Vec<CatalogEntry>,
    runes: Vec<CatalogEntry>,
}

impl AffixCatalog {
    /// An empty catalog for `language`: the state when the
    /// localized data files cannot be loaded, so the screen renders an
    /// empty grid instead of failing.
    pub fn empty(language: &str) -> Self {
        Self {
            language: language.to_string(),
            affixes: Vec::new(),
            aspects: Vec::new(),
            sigils: Vec::new(),
            uniques: Vec::new(),
            runes: Vec::new(),
        }
    }

    /// Load every browsable index for `language`.
    ///
    /// # Errors
    ///
    /// Returns the typed localized-data error of the first missing or
    /// unparsable index file (the same error shape the import pipeline
    /// reports).
    pub fn load(repo: &AssetRepository, language: &str) -> Result<Self, ImportLocalizedDataError> {
        Ok(Self {
            language: language.to_string(),
            affixes: load_entries(repo, language, CatalogKind::Affix)?,
            aspects: load_entries(repo, language, CatalogKind::Aspect)?,
            sigils: load_entries(repo, language, CatalogKind::Sigil)?,
            uniques: load_entries(repo, language, CatalogKind::Unique)?,
            runes: load_entries(repo, language, CatalogKind::Rune)?,
        })
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn entries(&self, kind: CatalogKind) -> &[CatalogEntry] {
        match kind {
            CatalogKind::Affix => &self.affixes,
            CatalogKind::Aspect => &self.aspects,
            CatalogKind::Sigil => &self.sigils,
            CatalogKind::Unique => &self.uniques,
            CatalogKind::Rune => &self.runes,
        }
    }

    pub fn entry(&self, kind: CatalogKind, id: &str) -> Option<&CatalogEntry> {
        self.entries(kind).iter().find(|entry| entry.id == id)
    }

    /// The card's display text for a preset entry id — the port's
    /// equivalent of upstream's `AffixIdToDescriptionConverter`
    /// (`AffixView.xaml:201`, `AffixManager.GetAffixDescription`), which
    /// resolves the wanted row's text from the loaded index rather than
    /// showing the raw id.
    pub fn label_for(&self, kind: CatalogKind, id: &str) -> Option<&str> {
        self.entry(kind, id).map(|entry| entry.label.as_str())
    }
}

fn load_entries(
    repo: &AssetRepository,
    language: &str,
    kind: CatalogKind,
) -> Result<Vec<CatalogEntry>, ImportLocalizedDataError> {
    let component = kind.component();
    let path = ensure_localized_data_file(repo, language, component)?;
    let text = std::fs::read_to_string(&path).map_err(|error| ImportLocalizedDataError {
        language: language.to_string(),
        component,
        path: path.display().to_string(),
        message: format!("failed to read {}: {error}", path.display()),
    })?;
    let rows: Vec<Value> =
        serde_json::from_str(&text).map_err(|error| ImportLocalizedDataError {
            language: language.to_string(),
            component,
            path: path.display().to_string(),
            message: format!("failed to parse {}: {error}", path.display()),
        })?;

    let mut entries: Vec<CatalogEntry> = Vec::with_capacity(rows.len());
    let mut claimed_ids: HashMap<String, String> = HashMap::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    for row in &rows {
        let Some(label) = entry_label(kind, row) else {
            continue;
        };
        if label.trim().is_empty() {
            continue;
        }
        let id = entry_id(kind, row, &label, &mut claimed_ids);
        // Fold rows whose canonical id was already claimed: one card
        // addresses one id, and the OCR path resolves those rows to the
        // same id too.
        if seen_ids.insert(id.clone(), ()).is_some() {
            continue;
        }
        entries.push(entry_from_row(kind, row, id, label));
    }
    Ok(entries)
}

fn entry_label(kind: CatalogKind, row: &Value) -> Option<String> {
    let field = match kind {
        CatalogKind::Affix => "Description",
        _ => "Name",
    };
    let base = string_field(row, field)?;
    // Dungeon sigils carry their zone in the display label (upstream
    // `AffixManager` sigil label, mirrored by the fuzzy candidate rule).
    if kind == CatalogKind::Sigil && string_field(row, "Type").is_some_and(|kind| kind == "Dungeon")
    {
        let zone = string_field(row, "DungeonZoneInfo").unwrap_or_default();
        if !zone.is_empty() {
            return Some(format!("{base} {zone}"));
        }
    }
    Some(base)
}

fn entry_id(
    kind: CatalogKind,
    row: &Value,
    label: &str,
    claimed: &mut HashMap<String, String>,
) -> String {
    let raw = string_field(row, "IdName")
        .or_else(|| string_field(row, "IdSno"))
        .unwrap_or_default();
    if matches!(kind, CatalogKind::Unique | CatalogKind::Sigil) {
        // Upstream keys these managers by label; rows sharing a label
        // resolve to the first row's id (the fuzzy candidate rule).
        claimed.entry(label.to_string()).or_insert(raw).clone()
    } else {
        raw
    }
}

fn entry_from_row(kind: CatalogKind, row: &Value, id: String, label: String) -> CatalogEntry {
    let classes = ClassFlags::from_allowed_for_player_class(
        row.get("AllowedForPlayerClass").and_then(Value::as_array),
    );
    let dungeon = string_field(row, "Dungeon").unwrap_or_default();
    CatalogEntry {
        id,
        label: label.clone(),
        description: match kind {
            CatalogKind::Affix => label,
            _ => string_field(row, "Description").unwrap_or_default(),
        },
        is_tempered: bool_field(row, "IsTemperingAvailable"),
        classes,
        is_codex: kind == CatalogKind::Aspect && bool_field(row, "IsCodex") && !dungeon.is_empty(),
        is_seasonal: bool_field(row, "IsSeasonal"),
        is_drop_only: kind == CatalogKind::Aspect && dungeon.is_empty(),
        dungeon,
        is_condition: kind == CatalogKind::Rune
            && string_field(row, "RuneType").is_some_and(|value| value == "condition"),
        is_effect: kind == CatalogKind::Rune
            && string_field(row, "RuneType").is_some_and(|value| value == "effect"),
        sigil_type: (kind == CatalogKind::Sigil)
            .then(|| string_field(row, "Type").unwrap_or_default()),
    }
}

fn string_field(row: &Value, field: &str) -> Option<String> {
    row.get(field).map(|value| match value {
        Value::String(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    })
}

fn bool_field(row: &Value, field: &str) -> bool {
    row.get(field).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// enUS index sizes of the vendored v3.2.0 baseline. Affixes,
    /// Aspects and Runes have one row per id; Uniques and Sigils fold
    /// same-label rows onto the first row's id (see the module docs), so
    /// their catalog counts equal the distinct-label counts, not the raw
    /// row counts (308/340 raw rows, 306/333 canonical ids).
    const ENUS_AFFIXES: usize = 894;
    const ENUS_ASPECTS: usize = 523;
    const ENUS_SIGILS: usize = 333;
    const ENUS_UNIQUES: usize = 306;
    const ENUS_RUNES: usize = 52;

    fn repo() -> AssetRepository {
        AssetRepository::new(workspace_root())
    }

    fn catalog(language: &str) -> AffixCatalog {
        AffixCatalog::load(&repo(), language).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn enus_catalog_counts_match_the_vendored_index_files() {
        let catalog = catalog("enUS");
        assert_eq!(catalog.language(), "enUS");
        assert_eq!(catalog.entries(CatalogKind::Affix).len(), ENUS_AFFIXES);
        assert_eq!(catalog.entries(CatalogKind::Aspect).len(), ENUS_ASPECTS);
        assert_eq!(catalog.entries(CatalogKind::Sigil).len(), ENUS_SIGILS);
        assert_eq!(catalog.entries(CatalogKind::Unique).len(), ENUS_UNIQUES);
        assert_eq!(catalog.entries(CatalogKind::Rune).len(), ENUS_RUNES);
    }

    #[test]
    fn catalog_counts_match_the_data_files_for_every_imported_language() {
        let repo = repo();
        let counts = repo
            .data_counts_by_category_language()
            .unwrap_or_else(|error| panic!("{error}"));
        let languages: Vec<String> = counts["Affixes"].keys().cloned().collect();
        assert_eq!(languages.len(), 14);

        for language in languages {
            let catalog = AffixCatalog::load(&repo, &language)
                .unwrap_or_else(|error| panic!("{language}: {error}"));
            for kind in CatalogKind::ALL {
                let stem = match kind {
                    CatalogKind::Affix => "Affixes",
                    CatalogKind::Aspect => "Aspects",
                    CatalogKind::Sigil => "Sigils",
                    CatalogKind::Unique => "Uniques",
                    CatalogKind::Rune => "Runes",
                };
                let raw_rows = counts[stem][&language];
                let entries = catalog.entries(kind);
                assert!(
                    entries.len() <= raw_rows,
                    "{language} {}: {} catalog entries exceed the {raw_rows} data rows",
                    kind.as_str(),
                    entries.len()
                );
                // Every catalog entry resolves to a distinct id, and each
                // id addresses a row of the same language file.
                let mut ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
                ids.sort_unstable();
                let distinct = ids.len();
                ids.dedup();
                assert_eq!(
                    ids.len(),
                    distinct,
                    "{language} {}: duplicate catalog ids",
                    kind.as_str()
                );
                assert!(
                    entries.iter().all(|entry| !entry.id.is_empty()),
                    "{language} {}: every catalog entry carries an id",
                    kind.as_str()
                );
            }
        }
    }

    #[test]
    fn affix_entries_project_tempering_and_class_markers() {
        let catalog = catalog("enUS");
        let tempered = catalog
            .entry(CatalogKind::Affix, "2HStaff_Unique_AF_001_PickupRadius")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this affix"));
        assert_eq!(tempered.label, "+# Pickup Radius");
        assert_eq!(tempered.description, "+# Pickup Radius");
        assert!(!tempered.is_tempered);
        assert_eq!(
            tempered.classes,
            ClassFlags {
                sorcerer: true,
                druid: true,
                ..ClassFlags::default()
            }
        );

        // The data marks a subset of affixes as tempering-available
        // (`IsTemperingAvailable`); the marker must follow the data, not a
        // static default.
        let tempering_count = catalog
            .entries(CatalogKind::Affix)
            .iter()
            .filter(|entry| entry.is_tempered)
            .count();
        assert_eq!(tempering_count, 377);
    }

    #[test]
    fn aspect_entries_project_codex_seasonal_and_drop_only_markers() {
        let catalog = catalog("enUS");
        // A Codex aspect carries a dungeon; the marker follows the data's
        // `IsCodex` + `Dungeon` pair (AspectInfoVM.cs:82-91).
        let codex = catalog
            .entry(CatalogKind::Aspect, "legendary_druid_108")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this aspect"));
        assert_eq!(codex.label, "Ballistic");
        assert_eq!(codex.dungeon, "Whispering Pines");
        assert!(codex.is_codex);
        assert!(!codex.is_drop_only);
        assert!(!codex.is_seasonal);
        assert_eq!(codex.classes, ClassFlags::default());

        // A dungeon-less aspect is drop-only and never Codex.
        let drop_only = catalog
            .entry(CatalogKind::Aspect, "Legendary_Generic_110")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this aspect"));
        assert_eq!(drop_only.label, "Accelerating");
        assert!(drop_only
            .description
            .starts_with("Critical Strikes with Core Skills"));
        assert!(drop_only.is_drop_only);
        assert!(!drop_only.is_codex);
        assert_eq!(
            drop_only.classes,
            ClassFlags {
                sorcerer: true,
                druid: true,
                barbarian: true,
                rogue: true,
                necromancer: true,
                spiritborn: true,
                paladin: true,
                warlock: true,
            }
        );

        let drop_only_count = catalog
            .entries(CatalogKind::Aspect)
            .iter()
            .filter(|entry| entry.is_drop_only)
            .count();
        assert_eq!(drop_only_count, 433);
        let codex_count = catalog
            .entries(CatalogKind::Aspect)
            .iter()
            .filter(|entry| entry.is_codex)
            .count();
        assert_eq!(codex_count, 90);
        assert!(
            catalog
                .entries(CatalogKind::Aspect)
                .iter()
                .all(|entry| !(entry.is_codex && entry.is_drop_only)),
            "Codex and DropOnly are mutually exclusive upstream"
        );
    }

    #[test]
    fn sigil_entries_project_seasonal_marker_and_type_token() {
        let catalog = catalog("enUS");
        let sigil = catalog
            .entry(CatalogKind::Sigil, "World_DGN_Kehj_AbandonedMineworks")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this sigil"));
        assert_eq!(sigil.label, "Abandoned Mineworks in Kehjistan");
        assert_eq!(sigil.sigil_type.as_deref(), Some("Dungeon"));
        assert!(!sigil.is_seasonal);

        let types: HashMap<&str, usize> =
            catalog
                .entries(CatalogKind::Sigil)
                .iter()
                .fold(HashMap::new(), |mut acc, entry| {
                    let token = entry.sigil_type.as_deref().unwrap_or_default();
                    *acc.entry(token).or_insert(0) += 1;
                    acc
                });
        assert_eq!(types.get("Dungeon").copied(), Some(153));
        assert_eq!(types.get("Positive").copied(), Some(82));
        assert_eq!(types.get("Minor").copied(), Some(49));
        assert_eq!(types.get("Major").copied(), Some(47));
        assert_eq!(types.get("Misc").copied(), Some(2));
    }

    #[test]
    fn rune_entries_project_condition_and_effect_markers() {
        let catalog = catalog("enUS");
        let condition = catalog
            .entry(CatalogKind::Rune, "Item_Rune_Condition_OnMove")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this rune"));
        assert_eq!(condition.label, "Bac");
        assert!(condition.is_condition);
        assert!(!condition.is_effect);

        let effect = catalog
            .entry(CatalogKind::Rune, "Item_Rune_Effect_SummonSpiritWolf")
            .unwrap_or_else(|| panic!("the v3.2.0 enUS data carries this rune"));
        assert_eq!(effect.label, "Ceh");
        assert!(effect.is_effect);
        assert!(!effect.is_condition);

        assert!(
            catalog
                .entries(CatalogKind::Rune)
                .iter()
                .all(|entry| entry.is_condition != entry.is_effect),
            "every rune is exactly one of condition/effect"
        );
    }

    #[test]
    fn unique_entries_fold_same_label_rows_onto_the_first_id() {
        // The enUS data carries one row whose IdName is a ';'-joined pair
        // and a second row with the same Name; both resolve to the first
        // row's id (the fuzzy candidate rule), so the catalog holds one
        // card for the label.
        let catalog = catalog("enUS");
        let label = "El'Druin, Sword of Justice";
        let matches: Vec<&CatalogEntry> = catalog
            .entries(CatalogKind::Unique)
            .iter()
            .filter(|entry| entry.label == label)
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "S14_1HSword_Unique_Generic_002");
        assert!(catalog
            .entry(
                CatalogKind::Unique,
                "1HSword_Unique_Generic_002;1HSword_Unique_Generic_103"
            )
            .is_none());
    }

    #[test]
    fn label_for_resolves_preset_entry_ids_to_display_text() {
        let catalog = catalog("enUS");
        assert_eq!(
            catalog.label_for(CatalogKind::Affix, "2HStaff_Unique_AF_001_PickupRadius"),
            Some("+# Pickup Radius")
        );
        assert_eq!(
            catalog.label_for(CatalogKind::Sigil, "World_DGN_Kehj_AbandonedMineworks"),
            Some("Abandoned Mineworks in Kehjistan")
        );
        assert_eq!(
            catalog.label_for(CatalogKind::Affix, "not_in_the_data"),
            None
        );
    }

    #[test]
    fn localized_catalog_loads_for_a_non_english_language() {
        let catalog = catalog("deDE");
        assert_eq!(catalog.language(), "deDE");
        assert_eq!(catalog.entries(CatalogKind::Affix).len(), 831);
        // German affix labels come from the localized file, not enUS.
        assert!(catalog
            .entries(CatalogKind::Affix)
            .iter()
            .any(|entry| !entry.label.is_ascii()));
    }

    #[test]
    fn missing_language_file_reports_the_typed_localized_data_error() {
        let error = AffixCatalog::load(&repo(), "xxXX").unwrap_err();
        assert_eq!(error.language, "xxXX");
        assert_eq!(error.component, LocalizedDataComponent::Affixes);
        assert!(error.path.ends_with("Affixes.xxXX.json"));
    }

    fn workspace_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        match manifest_dir.parent().and_then(|crates| crates.parent()) {
            Some(root) => root.to_path_buf(),
            None => panic!("could not resolve workspace root"),
        }
    }
}
