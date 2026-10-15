//! Affix-screen catalog projection: upstream's per-tab filtered, sorted
//! game-data views merged with the selected preset's wanted state.
//!
//! ## Upstream evidence
//!
//! - `AffixViewModel.cs:1096-1160` `FilterAffixes` (keyword split on `;`,
//!   AND-combined, plus the class-toggle OR arms)
//! - `AffixViewModel.cs:1176-1241` `FilterAspects`, `:1257-1292`
//!   `FilterSigils` (type toggles), `:1308-1373` `FilterUniques`,
//!   `:1389-1418` `FilterRunes`
//! - `AffixViewModel.cs:1082-1093,1162-1173,1243-1254,1294-1305,1375-1386`
//!   `CustomSort` wiring; `AffixInfoVM.cs:131-152` and the sibling
//!   `*InfoCustomSort` classes order the catalog by the card's display
//!   key with the Config cell first.
//!
//! The Config cell stays first because the screen renders it as the first
//! grid cell (`AffixView.xaml:1499-1520`), so the projection only orders
//! the entries by the kind's display key.
//!
//! ## Boundary rules
//!
//! Pure projection: no Slint handles, no file I/O, no runtime state.

use d4lc_config::RuleToggleSettings;
use d4lc_rules::{AffixCatalog, CatalogEntry, CatalogKind};

use crate::PresetEntry;

/// The screen's live filter state: the keyword text (upstream
/// `AffixTextFilter`) plus the 15 upstream toggles
/// (`AffixViewModel.cs:1114-1157,1195-1238,1270-1289,1327-1370,1408-1415`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogFilter {
    keywords: Vec<String>,
    core: bool,
    barbarian: bool,
    druid: bool,
    necromancer: bool,
    paladin: bool,
    rogue: bool,
    sorcerer: bool,
    spiritborn: bool,
    warlock: bool,
    dungeons: bool,
    positive: bool,
    minor: bool,
    major: bool,
    rune_condition: bool,
    rune_effect: bool,
}

impl Default for CatalogFilter {
    /// Upstream compiled defaults: every class/category/rune toggle is
    /// active and the filter box is empty (`SettingsD4.cs` defaults,
    /// mirrored by `RuleToggleSettings::default`).
    fn default() -> Self {
        Self::from_toggles("", &RuleToggleSettings::default())
    }
}

impl CatalogFilter {
    /// Build the filter from the raw filter text and the persisted toggle
    /// settings.
    pub fn from_toggles(text: &str, toggles: &RuleToggleSettings) -> Self {
        let class = &toggles.class_toggles;
        Self {
            keywords: split_keywords(text),
            core: class.core,
            barbarian: class.barbarian,
            druid: class.druid,
            necromancer: class.necromancer,
            paladin: class.paladin,
            rogue: class.rogue,
            sorcerer: class.sorcerer,
            spiritborn: class.spiritborn,
            warlock: class.warlock,
            dungeons: toggles.dungeons,
            positive: toggles.positive,
            minor: toggles.minor,
            major: toggles.major,
            rune_condition: toggles.rune_condition,
            rune_effect: toggles.rune_effect,
        }
    }

    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Whether `entry` passes the filter for its tab.
    pub fn matches(&self, kind: CatalogKind, entry: &CatalogEntry) -> bool {
        match kind {
            CatalogKind::Affix => self.matches_affix(entry),
            CatalogKind::Aspect => self.matches_aspect(entry),
            CatalogKind::Sigil => self.matches_sigil(entry),
            CatalogKind::Unique => self.matches_unique(entry),
            CatalogKind::Rune => self.matches_rune(entry),
        }
    }

    fn matches_affix(&self, entry: &CatalogEntry) -> bool {
        // Upstream matches the affix `Description` only
        // (AffixViewModel.cs:1108); the catalog affix entry's description
        // is that same data field.
        if !self.keywords_match(&[entry.description.as_str()]) {
            return false;
        }
        self.class_matches(entry)
    }

    fn matches_aspect(&self, entry: &CatalogEntry) -> bool {
        // Description or Name (AffixViewModel.cs:1188-1192).
        if !self.keywords_match(&[entry.description.as_str(), entry.label.as_str()]) {
            return false;
        }
        self.class_matches(entry)
    }

    fn matches_unique(&self, entry: &CatalogEntry) -> bool {
        // Description or Name (AffixViewModel.cs:1320-1324).
        if !self.keywords_match(&[entry.description.as_str(), entry.label.as_str()]) {
            return false;
        }
        self.class_matches(entry)
    }

    fn matches_sigil(&self, entry: &CatalogEntry) -> bool {
        // Description or Name (AffixViewModel.cs:1265-1268).
        if !self.keywords_match(&[entry.description.as_str(), entry.label.as_str()]) {
            return false;
        }
        // The sigil type selects exactly one category toggle; an unknown
        // type token is never allowed (AffixViewModel.cs:1270-1289).
        match entry.sigil_type.as_deref() {
            Some("Dungeon") => self.dungeons,
            Some("Positive") => self.positive,
            Some("Major") => self.major,
            Some("Minor") => self.minor,
            _ => false,
        }
    }

    fn matches_rune(&self, entry: &CatalogEntry) -> bool {
        // Description or Name (AffixViewModel.cs:1402).
        if !self.keywords_match(&[entry.description.as_str(), entry.label.as_str()]) {
            return false;
        }
        if entry.is_condition {
            return self.rune_condition;
        }
        if entry.is_effect {
            return self.rune_effect;
        }
        // Upstream leaves a rune that is neither condition nor effect
        // allowed (`allowed = true` default, AffixViewModel.cs:1391).
        true
    }

    /// Upstream class arms (`AffixViewModel.cs:1114-1157`): "Core" matches
    /// an unrestricted entry (all classes 1 or all 0), every other arm
    /// matches an entry restricted to that class but not to all classes.
    /// An entry passes when any enabled arm matches.
    fn class_matches(&self, entry: &CatalogEntry) -> bool {
        let classes = entry.classes;
        let all_classes = classes.all();
        if self.core && (all_classes || classes.none()) {
            return true;
        }
        let restricted_to = |enabled: bool, flag: bool| enabled && flag && !all_classes;
        restricted_to(self.barbarian, classes.barbarian)
            || restricted_to(self.druid, classes.druid)
            || restricted_to(self.necromancer, classes.necromancer)
            || restricted_to(self.paladin, classes.paladin)
            || restricted_to(self.rogue, classes.rogue)
            || restricted_to(self.sorcerer, classes.sorcerer)
            || restricted_to(self.spiritborn, classes.spiritborn)
            || restricted_to(self.warlock, classes.warlock)
    }

    /// `;`-split keywords AND-combine; blank terms are ignored and every
    /// term must appear in one of the entry's text fields
    /// (AffixViewModel.cs:1103-1112).
    fn keywords_match(&self, fields: &[&str]) -> bool {
        self.keywords.iter().all(|keyword| {
            let needle = keyword.trim().to_lowercase();
            fields
                .iter()
                .any(|field| field.to_lowercase().contains(&needle))
        })
    }
}

fn split_keywords(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(str::to_string)
        .collect()
}

/// One projected catalog row: the game-data entry plus the selected
/// preset's wanted entry when the id is already wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRow {
    pub entry: CatalogEntry,
    pub wanted: Option<PresetEntry>,
}

impl CatalogRow {
    /// The row's swatch / card color: the wanted entry's color when the
    /// id is wanted, `None` otherwise (the card then renders no color
    /// swatch — upstream's catalog cards carry no color control,
    /// `AffixView.xaml:1538-1587`).
    pub fn wanted_color(&self) -> Option<&str> {
        self.wanted.as_ref().map(|entry| entry.color.as_str())
    }

    /// The wanted entry's item type (the slot the entry was added for),
    /// shown as the card's secondary line.
    pub fn wanted_item_type(&self) -> &str {
        self.wanted
            .as_ref()
            .and_then(|entry| entry.item_type.as_deref())
            .unwrap_or("")
    }

    /// The wanted entry's display name — the left rail row's text: the
    /// user-entered label when present, the catalog's display text
    /// otherwise (upstream's `AffixIdToDescriptionConverter`), and `None`
    /// while the id is not wanted.
    pub fn wanted_display_name(&self) -> Option<&str> {
        let wanted = self.wanted.as_ref()?;
        Some(wanted.label.as_deref().unwrap_or(self.entry.label.as_str()))
    }
}

/// Project one tab's catalog: filter, merge the wanted state, then sort
/// with the kind's `CustomSort` key.
pub fn project(
    kind: CatalogKind,
    catalog: &AffixCatalog,
    preset_entries: &[PresetEntry],
    filter: &CatalogFilter,
) -> Vec<CatalogRow> {
    let mut rows: Vec<CatalogRow> = catalog
        .entries(kind)
        .iter()
        .filter(|entry| filter.matches(kind, entry))
        .map(|entry| CatalogRow {
            entry: entry.clone(),
            wanted: wanted_entry(preset_entries, kind, &entry.id).cloned(),
        })
        .collect();
    rows.sort_by(|left, right| {
        compare_sort_keys(sort_key(kind, &left.entry), sort_key(kind, &right.entry))
    });
    rows
}

/// The upstream `CustomSort` key: affixes order by `Description`
/// (`AffixInfoVM.cs:147`), every other kind by `Name`
/// (`AspectInfoVM.cs:139`, `SigilInfoVM.cs:152`, `UniqueInfoVM.cs:147`,
/// `RuneInfoVM.cs:113`).
pub fn sort_key(kind: CatalogKind, entry: &CatalogEntry) -> &str {
    match kind {
        CatalogKind::Affix => entry.description.as_str(),
        _ => entry.label.as_str(),
    }
}

/// Compare two sort keys the way .NET's culture-aware
/// `string.CompareTo` orders the shipped data: primary letter weights
/// (case-insensitive), with the raw string as a deterministic tiebreak.
///
/// Accented letters are not folded onto their base letters, so a
/// non-English ordering can differ from .NET's ICU collation in the
/// accented range; the primary order and the tiebreak are otherwise
/// identical.
fn compare_sort_keys(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

/// The preset entry a catalog id is wanted by.
///
/// The key is (kind, id): the non-affix kinds project one entry per id
/// (the preset's `item_type` is the "any slot" filter), and an affix id
/// can be wanted for several slots — the first match carries the card's
/// swatch and item type.
fn wanted_entry<'a>(
    preset_entries: &'a [PresetEntry],
    kind: CatalogKind,
    id: &str,
) -> Option<&'a PresetEntry> {
    preset_entries
        .iter()
        .find(|entry| entry.kind == preset_kind(kind) && entry.id == id)
}

fn preset_kind(kind: CatalogKind) -> crate::PresetEntryKind {
    match kind {
        CatalogKind::Affix => crate::PresetEntryKind::Affix,
        CatalogKind::Aspect => crate::PresetEntryKind::Aspect,
        CatalogKind::Sigil => crate::PresetEntryKind::Sigil,
        CatalogKind::Unique => crate::PresetEntryKind::Unique,
        CatalogKind::Rune => crate::PresetEntryKind::Rune,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use d4lc_assets::AssetRepository;
    use std::path::PathBuf;

    /// The vendored v3.2.0 enUS index sizes the catalog projects (the
    /// Uniques/Sigils counts fold same-label rows onto the first row's id,
    /// see `d4lc_rules::catalog`).
    const ENUS_AFFIXES: usize = 894;
    const ENUS_ASPECTS: usize = 523;
    /// Canonical sigil ids minus the two `Misc` rows upstream never
    /// allows (AffixViewModel.cs:1286-1289).
    const ENUS_SIGILS_FILTERED: usize = 331;
    const ENUS_RUNES: usize = 52;

    fn catalog() -> AffixCatalog {
        let repo = AssetRepository::new(workspace_root());
        AffixCatalog::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"))
    }

    fn project_all(kind: CatalogKind, filter: &CatalogFilter) -> Vec<CatalogRow> {
        project(kind, &catalog(), &[], filter)
    }

    #[test]
    fn enus_affix_projection_lists_the_full_catalog_in_sort_order() {
        let rows = project_all(CatalogKind::Affix, &CatalogFilter::default());
        assert_eq!(rows.len(), ENUS_AFFIXES);
        assert!(
            rows.windows(2).all(|pair| {
                compare_sort_keys(&pair[0].entry.description, &pair[1].entry.description)
                    != std::cmp::Ordering::Greater
            }),
            "affix rows must ascend by Description (AffixInfoVM.cs:147)"
        );
    }

    #[test]
    fn every_tab_projects_its_full_catalog() {
        let catalog = catalog();
        let filter = CatalogFilter::default();
        assert_eq!(
            project(CatalogKind::Affix, &catalog, &[], &filter).len(),
            ENUS_AFFIXES
        );
        assert_eq!(
            project(CatalogKind::Aspect, &catalog, &[], &filter).len(),
            ENUS_ASPECTS
        );
        assert_eq!(
            project(CatalogKind::Sigil, &catalog, &[], &filter).len(),
            ENUS_SIGILS_FILTERED
        );
        assert_eq!(
            project(CatalogKind::Unique, &catalog, &[], &filter).len(),
            306
        );
        assert_eq!(
            project(CatalogKind::Rune, &catalog, &[], &filter).len(),
            ENUS_RUNES
        );
        // The non-affix tabs order by Name, not Description.
        for kind in [
            CatalogKind::Aspect,
            CatalogKind::Sigil,
            CatalogKind::Unique,
            CatalogKind::Rune,
        ] {
            let rows = project(kind, &catalog, &[], &filter);
            assert!(
                rows.windows(2).all(|pair| {
                    compare_sort_keys(&pair[0].entry.label, &pair[1].entry.label)
                        != std::cmp::Ordering::Greater
                }),
                "{kind:?} rows must ascend by Name"
            );
        }
    }

    #[test]
    fn keyword_filter_narrows_live_and_combines_terms() {
        let catalog = catalog();
        let unfiltered = project(CatalogKind::Affix, &catalog, &[], &CatalogFilter::default());
        assert_eq!(unfiltered.len(), ENUS_AFFIXES);

        let pickup = CatalogFilter::from_toggles("pickup", &RuleToggleSettings::default());
        let rows = project(CatalogKind::Affix, &catalog, &[], &pickup);
        assert!(!rows.is_empty());
        assert!(rows.len() < unfiltered.len());
        assert!(rows
            .iter()
            .all(|row| row.entry.description.to_lowercase().contains("pickup")));

        // Two terms AND-combine: the second term narrows the first result.
        let pickup_radius =
            CatalogFilter::from_toggles("pickup;radius", &RuleToggleSettings::default());
        let narrowed = project(CatalogKind::Affix, &catalog, &[], &pickup_radius);
        assert!(!narrowed.is_empty());
        assert!(narrowed.len() <= rows.len());
        assert!(narrowed.iter().all(|row| {
            row.entry.description.to_lowercase().contains("pickup")
                && row.entry.description.to_lowercase().contains("radius")
        }));

        // A term that matches nothing yields the empty grid.
        let none = CatalogFilter::from_toggles("pickup;zzzzzz", &RuleToggleSettings::default());
        assert!(project(CatalogKind::Affix, &catalog, &[], &none).is_empty());

        // Clearing restores the full catalog.
        let cleared = CatalogFilter::from_toggles("  ;  ", &RuleToggleSettings::default());
        assert_eq!(
            project(CatalogKind::Affix, &catalog, &[], &cleared).len(),
            ENUS_AFFIXES
        );
    }

    #[test]
    fn aspect_filter_matches_name_or_description() {
        let catalog = catalog();
        let filter = CatalogFilter::from_toggles("Accelerating", &RuleToggleSettings::default());
        let rows = project(CatalogKind::Aspect, &catalog, &[], &filter);
        assert!(rows.iter().any(|row| row.entry.label == "Accelerating"));

        let filter = CatalogFilter::from_toggles(
            "Critical Strikes with Core",
            &RuleToggleSettings::default(),
        );
        let rows = project(CatalogKind::Aspect, &catalog, &[], &filter);
        assert!(!rows.is_empty());
        assert!(rows
            .iter()
            .all(|row| row.entry.description.contains("Critical Strikes with Core")));
    }

    #[test]
    fn class_toggles_follow_the_upstream_or_arms_on_real_data() {
        let catalog = catalog();

        // Only the barbarian arm: every row is barbarian-restricted and
        // not allowed for all classes.
        let mut toggles = RuleToggleSettings::default();
        toggles.class_toggles.core = false;
        for class in [
            &mut toggles.class_toggles.druid,
            &mut toggles.class_toggles.necromancer,
            &mut toggles.class_toggles.paladin,
            &mut toggles.class_toggles.rogue,
            &mut toggles.class_toggles.sorcerer,
            &mut toggles.class_toggles.spiritborn,
            &mut toggles.class_toggles.warlock,
        ] {
            *class = false;
        }
        let filter = CatalogFilter::from_toggles("", &toggles);
        let rows = project(CatalogKind::Affix, &catalog, &[], &filter);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row.entry.classes.barbarian
            && !row.entry.classes.all()
            && !row.entry.classes.none()));

        // Only the core arm: every row is unrestricted (all classes or no
        // class marker at all).
        let mut toggles = RuleToggleSettings::default();
        toggles.class_toggles.barbarian = false;
        toggles.class_toggles.druid = false;
        toggles.class_toggles.necromancer = false;
        toggles.class_toggles.paladin = false;
        toggles.class_toggles.rogue = false;
        toggles.class_toggles.sorcerer = false;
        toggles.class_toggles.spiritborn = false;
        toggles.class_toggles.warlock = false;
        let filter = CatalogFilter::from_toggles("", &toggles);
        let rows = project(CatalogKind::Affix, &catalog, &[], &filter);
        assert!(!rows.is_empty());
        assert!(rows
            .iter()
            .all(|row| row.entry.classes.all() || row.entry.classes.none()));

        // Turning every arm off empties the class-filtered tabs.
        let mut toggles = RuleToggleSettings::default();
        let class = &mut toggles.class_toggles;
        class.core = false;
        class.barbarian = false;
        class.druid = false;
        class.necromancer = false;
        class.paladin = false;
        class.rogue = false;
        class.sorcerer = false;
        class.spiritborn = false;
        class.warlock = false;
        let filter = CatalogFilter::from_toggles("", &toggles);
        assert!(project(CatalogKind::Affix, &catalog, &[], &filter).is_empty());
    }

    #[test]
    fn sigil_category_toggles_select_one_type_and_hide_misc() {
        let catalog = catalog();
        let rows = project(CatalogKind::Sigil, &catalog, &[], &CatalogFilter::default());
        assert_eq!(rows.len(), ENUS_SIGILS_FILTERED);
        assert!(rows.iter().all(|row| matches!(
            row.entry.sigil_type.as_deref(),
            Some("Dungeon" | "Positive" | "Minor" | "Major")
        )));

        let toggles = RuleToggleSettings {
            dungeons: false,
            positive: false,
            minor: false,
            major: false,
            ..RuleToggleSettings::default()
        };
        let filter = CatalogFilter::from_toggles("", &toggles);
        assert!(project(CatalogKind::Sigil, &catalog, &[], &filter).is_empty());
    }

    #[test]
    fn rune_toggles_split_conditions_from_effects() {
        let catalog = catalog();
        let toggles = RuleToggleSettings {
            rune_effect: false,
            ..RuleToggleSettings::default()
        };
        let filter = CatalogFilter::from_toggles("", &toggles);
        let rows = project(CatalogKind::Rune, &catalog, &[], &filter);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row.entry.is_condition));

        let toggles = RuleToggleSettings {
            rune_condition: false,
            ..RuleToggleSettings::default()
        };
        let filter = CatalogFilter::from_toggles("", &toggles);
        let rows = project(CatalogKind::Rune, &catalog, &[], &filter);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row.entry.is_effect));
    }

    #[test]
    fn projection_marks_wanted_ids_with_their_color_and_item_type() {
        let catalog = catalog();
        // A real enUS affix wanted for one slot: the catalog row carries
        // the preset entry's color and the slot the user added it for.
        let preset = vec![PresetEntry {
            kind: crate::PresetEntryKind::Affix,
            id: "2HStaff_Unique_AF_001_PickupRadius".to_string(),
            label: None,
            item_type: Some("Amulet".to_string()),
            color: "#22C55E".to_string(),
            is_any_type: false,
            is_greater: true,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }];
        let rows = project(
            CatalogKind::Affix,
            &catalog,
            &preset,
            &CatalogFilter::default(),
        );
        let wanted = rows
            .iter()
            .find(|row| row.entry.id == "2HStaff_Unique_AF_001_PickupRadius")
            .expect("the wanted affix is in the catalog");
        assert_eq!(wanted.wanted_color(), Some("#22C55E"));
        assert_eq!(wanted.wanted_item_type(), "Amulet");
        assert_eq!(
            wanted.wanted.as_ref().map(|entry| entry.is_greater),
            Some(true)
        );
        assert!(rows.iter().filter(|row| row.wanted.is_some()).count() == 1);

        // A same-id entry of a foreign kind never marks the row.
        let preset = vec![PresetEntry::non_affix(
            crate::PresetEntryKind::Unique,
            "2HStaff_Unique_AF_001_PickupRadius",
            None,
            "#008000",
        )];
        let rows = project(
            CatalogKind::Affix,
            &catalog,
            &preset,
            &CatalogFilter::default(),
        );
        assert!(rows.iter().all(|row| row.wanted.is_none()));
    }

    #[test]
    fn filter_is_reusable_across_tabs_and_keeps_the_wanted_state() {
        // The screen keeps one filter per keystroke and re-projects every
        // tab from the same catalog + preset, so a toggle/tab switch never
        // needs a reload.
        let catalog = catalog();
        let toggles = RuleToggleSettings {
            dungeons: false,
            ..RuleToggleSettings::default()
        };
        let filter = CatalogFilter::from_toggles("dungeon", &toggles);
        let preset = vec![PresetEntry::non_affix(
            crate::PresetEntryKind::Sigil,
            "World_DGN_Kehj_AbandonedMineworks",
            None,
            "#008000",
        )];
        let sigils = project(CatalogKind::Sigil, &catalog, &preset, &filter);
        assert!(
            sigils
                .iter()
                .all(|row| row.entry.sigil_type.as_deref() != Some("Dungeon")),
            "dungeons off hides the dungeon sigils"
        );
        assert!(
            sigils
                .iter()
                .all(|row| row.entry.id != "World_DGN_Kehj_AbandonedMineworks"),
            "the wanted dungeon sigil leaves the grid with its category toggle"
        );
        // The same filter on another tab selects from that tab's entries.
        let aspects = project(CatalogKind::Aspect, &catalog, &preset, &filter);
        assert!(aspects.iter().all(|row| {
            row.entry.description.to_lowercase().contains("dungeon")
                || row.entry.label.to_lowercase().contains("dungeon")
        }));
    }

    fn workspace_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        match manifest_dir.parent().and_then(|crates| crates.parent()) {
            Some(root) => root.to_path_buf(),
            None => panic!("could not resolve workspace root"),
        }
    }
}
