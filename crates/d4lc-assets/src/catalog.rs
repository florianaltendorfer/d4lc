//! Canonical identity space of the pinned catalog state.
//!
//! One affix identity is the complete `IdName` string of the pinned
//! `Affixes.{language}.json` row. The single `IdNameList` names and the
//! `IdSnoList` SNOs of that row are lookup keys onto that identity — never
//! competing result ids. Every consumer has to end up on the resolved
//! `IdName`:
//!
//! - the fuzzy OCR index keys its candidates by the row `IdName`
//!   (`OcrHandler.TextToAffix`), so the parsed tooltip row already carries it;
//! - upstream imports store it (`BuildsManagerD4Builds.ConvertItemAffix`
//!   resolves the affix text through `_affixMapDescriptionToId`, which maps
//!   the description to `affix.IdName`; `BuildsManagerMaxroll` and
//!   `BuildsManagerD2Core` store `affixInfo.IdName` from
//!   `AffixManager.GetAffixInfoMaxrollByIdSno` /
//!   `AffixManager.GetAffixInfoByIdName`, both `FirstOrDefault` over
//!   `IdSnoList` / `IdNameList` membership);
//! - the evaluation compares ids exactly (`AffixManager.GetAffix`:
//!   `a.Id.Equals(affixId) && a.Type.Equals(itemType)`).
//!
//! Item type identities are the catalog `Type` values (`helm`, `chest`,
//! `weapon`, ...). The per-rarity `Name` strings ("Legendary Helm") are
//! display labels only; the evaluation id is the canonical `Type` value that
//! `OcrHandler.ConvertToItemType` resolves and that imports carry.
//!
//! Ambiguity is reported, never guessed: a lookup key that carries several
//! identities comes back as [`IdentityResolution::Ambiguous`] with every
//! candidate listed in catalog order. The provider import lookups keep the
//! upstream `FirstOrDefault` behavior through [`IdentityCatalog::first_match`]
//! — which is only unambiguous because the pinned catalog state has no
//! ambiguous affix keys (see the catalog tests).

use crate::AssetRepository;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;

/// The affix label field the OCR fuzzy index and the D4Builds provider match
/// on (`OcrHandler.TextToAffix`, `BuildsManagerD4Builds.GetAllAffixes`).
pub const AFFIX_LABEL_FIELD: &str = "DescriptionClean";

/// One catalog row's canonical identity plus every lookup key that resolves
/// onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRowIdentity {
    /// Canonical identity: the complete `IdName` string of the pinned row.
    pub id_name: String,
    /// The single names the row carries (`IdNameList`), in catalog order.
    /// Rows without an `IdNameList` resolve through their `IdName` segments.
    pub aliases: Vec<String>,
    /// The SNOs the row carries (`IdSnoList`, falling back to the `IdSno`
    /// list), in catalog order.
    pub snos: Vec<String>,
    /// The row's localized label field value (`DescriptionClean` for affixes,
    /// `Name` for aspects/uniques/runes/sigils). `None` when the row does not
    /// carry the field; a present-but-blank value is kept as `Some("")` so
    /// every consumer keeps its own blank-label rule.
    pub label: Option<String>,
}

/// Outcome of resolving a lookup key onto the canonical identity space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityResolution<T> {
    /// Exactly one canonical identity carries the key.
    Resolved(T),
    /// Several canonical identities carry the key, listed in catalog order.
    Ambiguous(Vec<T>),
    /// No canonical identity carries the key.
    Unknown,
}

impl<T> IdentityResolution<T> {
    /// The resolved value, or `None` for an unknown or ambiguous key.
    ///
    /// Callers that must not guess use this and surface the unresolved key;
    /// only the upstream-parity provider lookups take the first candidate
    /// ([`IdentityCatalog::first_match`]).
    pub fn resolved(self) -> Option<T> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::Ambiguous(_) | Self::Unknown => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// Canonical identity index of one localized catalog category file.
///
/// Rows keep catalog order, which is the order the upstream `FirstOrDefault`
/// lookups observe.
#[derive(Debug, Clone)]
pub struct IdentityCatalog {
    rows: Vec<CatalogRowIdentity>,
    by_id_name: HashMap<String, Vec<usize>>,
    by_alias: HashMap<String, Vec<usize>>,
    by_sno: HashMap<String, Vec<usize>>,
}

impl IdentityCatalog {
    /// The canonical affix identity space: the complete `IdName` of the
    /// pinned `Affixes.{language}.json` state, reachable through every
    /// `IdNameList` name and every `IdSnoList` SNO.
    pub fn affixes(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        Self::load(repo, language, "Affixes", AFFIX_LABEL_FIELD)
    }

    /// Load the canonical identity index of one localized catalog category.
    ///
    /// `label_field` is the row's localized label field (`DescriptionClean`
    /// for affixes, `Name` for the non-affix categories).
    pub fn load(
        repo: &AssetRepository,
        language: &str,
        category: &str,
        label_field: &str,
    ) -> Result<Self, String> {
        let path = repo.data_dir().join(format!("{category}.{language}.json"));
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let items: Vec<Value> = serde_json::from_str(&text)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        Ok(Self::from_items(&items, label_field))
    }

    /// Build the identity index from already-parsed catalog rows. Rows without
    /// an `IdName` carry no canonical identity and are skipped.
    pub fn from_items(items: &[Value], label_field: &str) -> Self {
        let mut rows = Vec::new();
        let mut by_id_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_alias: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_sno: HashMap<String, Vec<usize>> = HashMap::new();

        for item in items {
            let Some(id_name) = string_field(item, "IdName") else {
                continue;
            };
            if id_name.is_empty() {
                continue;
            }
            let index = rows.len();
            by_id_name.entry(id_name.clone()).or_default().push(index);

            let aliases = string_list_field(item, "IdNameList")
                .unwrap_or_else(|| id_name.split(';').map(ToOwned::to_owned).collect());
            for alias in &aliases {
                by_alias.entry(alias.clone()).or_default().push(index);
            }

            let snos = string_list_field(item, "IdSnoList").unwrap_or_else(|| {
                string_field(item, "IdSno")
                    .map(|sno| sno.split(';').map(ToOwned::to_owned).collect())
                    .unwrap_or_default()
            });
            for sno in &snos {
                by_sno.entry(sno.clone()).or_default().push(index);
            }

            rows.push(CatalogRowIdentity {
                id_name,
                aliases,
                snos,
                label: string_field(item, label_field),
            });
        }

        Self {
            rows,
            by_id_name,
            by_alias,
            by_sno,
        }
    }

    /// Rows in catalog order.
    pub fn rows(&self) -> &[CatalogRowIdentity] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Resolve an exact canonical `IdName`.
    pub fn resolve_id_name(&self, id_name: &str) -> IdentityResolution<&CatalogRowIdentity> {
        self.resolve_indexes(self.by_id_name.get(id_name).map(Vec::as_slice))
    }

    /// Resolve a single `IdNameList` name onto its canonical identity.
    pub fn resolve_alias(&self, alias: &str) -> IdentityResolution<&CatalogRowIdentity> {
        self.resolve_indexes(self.by_alias.get(alias).map(Vec::as_slice))
    }

    /// Resolve an `IdSnoList` SNO onto its canonical identity.
    pub fn resolve_sno(&self, sno: &str) -> IdentityResolution<&CatalogRowIdentity> {
        self.resolve_indexes(self.by_sno.get(sno).map(Vec::as_slice))
    }

    /// Resolve any lookup key: an exact canonical `IdName` first, then the
    /// alias and SNO keys. A key that carries several identities — including
    /// one that is an alias of one row and a SNO of another — is reported as
    /// ambiguous with every candidate.
    pub fn resolve(&self, key: &str) -> IdentityResolution<&CatalogRowIdentity> {
        match self.by_id_name.get(key) {
            Some(indexes) => self.resolve_indexes(Some(indexes)),
            None => {
                let mut indexes = Vec::new();
                for map in [&self.by_alias, &self.by_sno] {
                    for index in map.get(key).into_iter().flatten() {
                        if !indexes.contains(index) {
                            indexes.push(*index);
                        }
                    }
                }
                indexes.sort_unstable();
                match indexes.as_slice() {
                    [] => IdentityResolution::Unknown,
                    [index] => IdentityResolution::Resolved(&self.rows[*index]),
                    _ => IdentityResolution::Ambiguous(
                        indexes.iter().map(|index| &self.rows[*index]).collect(),
                    ),
                }
            }
        }
    }

    /// Upstream `FirstOrDefault` parity for the provider import lookups: the
    /// first row in catalog order whose canonical id, alias, or SNO is `key`.
    ///
    /// The pinned catalog state resolves every affix key to exactly one
    /// identity (asserted by the catalog tests); callers that must keep an
    /// ambiguous key visible use [`Self::resolve`] instead.
    pub fn first_match(&self, key: &str) -> Option<&CatalogRowIdentity> {
        if let Some(index) = self.by_id_name.get(key).and_then(|indexes| indexes.first()) {
            return Some(&self.rows[*index]);
        }
        self.by_alias
            .get(key)
            .into_iter()
            .chain(self.by_sno.get(key))
            .flatten()
            .min()
            .map(|index| &self.rows[*index])
    }

    fn resolve_indexes(
        &self,
        indexes: Option<&[usize]>,
    ) -> IdentityResolution<&CatalogRowIdentity> {
        match indexes.unwrap_or_default() {
            [] => IdentityResolution::Unknown,
            [index] => IdentityResolution::Resolved(&self.rows[*index]),
            indexes => IdentityResolution::Ambiguous(
                indexes.iter().map(|index| &self.rows[*index]).collect(),
            ),
        }
    }
}

/// One pinned `ItemTypes` row: the canonical `Type` id and one display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTypeRow {
    /// Canonical item type id (`helm`, `chest`, `weapon`, ...).
    pub type_id: String,
    /// Localized display label of this rarity row ("Legendary Helm").
    pub name: String,
    /// Rarity of this label row ("Legendary", "Magic", ...).
    pub rarity: String,
}

/// Canonical item type identity index of the pinned `ItemTypes.{language}.json`
/// state. The `Type` value is the evaluation id; the `Name` values are display
/// labels per rarity and never an evaluation id.
#[derive(Debug, Clone)]
pub struct ItemTypeCatalog {
    rows: Vec<ItemTypeRow>,
    by_type: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

impl ItemTypeCatalog {
    pub fn load(repo: &AssetRepository, language: &str) -> Result<Self, String> {
        let path = repo.data_dir().join(format!("ItemTypes.{language}.json"));
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let items: Vec<Value> = serde_json::from_str(&text)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        Ok(Self::from_items(&items))
    }

    pub fn from_items(items: &[Value]) -> Self {
        let mut rows = Vec::new();
        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();

        for item in items {
            let Some(type_id) = string_field(item, "Type") else {
                continue;
            };
            if type_id.is_empty() {
                continue;
            }
            let index = rows.len();
            by_type.entry(type_id.clone()).or_default().push(index);
            let name = string_field(item, "Name").unwrap_or_default();
            if !name.trim().is_empty() {
                by_name.entry(name.clone()).or_default().push(index);
            }
            rows.push(ItemTypeRow {
                type_id,
                name,
                rarity: string_field(item, "Rarerity").unwrap_or_default(),
            });
        }

        Self {
            rows,
            by_type,
            by_name,
        }
    }

    /// Label rows in catalog order.
    pub fn rows(&self) -> &[ItemTypeRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Canonical type ids in catalog order, deduplicated.
    pub fn type_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();
        for row in &self.rows {
            if !ids.contains(&row.type_id.as_str()) {
                ids.push(row.type_id.as_str());
            }
        }
        ids
    }

    /// True when `type_id` is a canonical catalog `Type` value — the only
    /// values rule evaluation may compare against.
    pub fn is_canonical(&self, type_id: &str) -> bool {
        self.by_type.contains_key(type_id)
    }

    /// Resolve a canonical `Type` value, or a display label onto its canonical
    /// type id. Ambiguous labels list every candidate id in catalog order.
    pub fn resolve(&self, key: &str) -> IdentityResolution<&str> {
        let indexes = if let Some(indexes) = self.by_type.get(key) {
            indexes
        } else if let Some(indexes) = self.by_name.get(key) {
            indexes
        } else {
            return IdentityResolution::Unknown;
        };
        let ids: Vec<&str> = indexes
            .iter()
            .map(|index| self.rows[*index].type_id.as_str())
            .collect();
        let mut unique: Vec<&str> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        match unique.as_slice() {
            [id] => IdentityResolution::Resolved(id),
            _ => IdentityResolution::Ambiguous(unique),
        }
    }

    /// Display label of one canonical type id for a rarity ("Legendary Helm"
    /// for `helm`), when the pinned catalog carries that row.
    pub fn display_label(&self, type_id: &str, rarity: &str) -> Option<&str> {
        let indexes = self.by_type.get(type_id)?;
        indexes
            .iter()
            .map(|index| &self.rows[*index])
            .find(|row| row.rarity == rarity && !row.name.trim().is_empty())
            .map(|row| row.name.as_str())
    }

    /// Every display label of one canonical type id, in catalog order.
    pub fn display_labels(&self, type_id: &str) -> Vec<&str> {
        self.by_type
            .get(type_id)
            .into_iter()
            .flatten()
            .map(|index| self.rows[*index].name.as_str())
            .collect()
    }
}

fn string_field(item: &Value, field: &str) -> Option<String> {
    match item.get(field)? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn string_list_field(item: &Value, field: &str) -> Option<Vec<String>> {
    let values = item.get(field)?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(|value| match value {
                Value::String(value) => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The composite-identity case: the pinned catalog
    /// records `AttackSpeed` as the first `IdNameList` alias of a composite
    /// `IdName`. The bare alias, the SNO and the exact `IdName` must all
    /// resolve to that one canonical identity.
    #[test]
    fn affix_catalog_resolves_alias_sno_and_id_name_to_one_identity() {
        let repo = AssetRepository::new(workspace_root());
        let catalog =
            IdentityCatalog::affixes(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(catalog.len(), 894);

        let by_alias = catalog
            .resolve_alias("AttackSpeed")
            .resolved()
            .unwrap_or_else(|| panic!("bare alias must resolve"));
        assert_ne!(by_alias.id_name, "AttackSpeed");
        assert!(
            by_alias.id_name.starts_with("AttackSpeed;"),
            "canonical identity is the composite IdName, got {}",
            by_alias.id_name
        );
        assert!(by_alias
            .id_name
            .contains("Tempered_Generic_AttackSpeed_Tier1"));
        assert!(by_alias.aliases.contains(&"AttackSpeed".to_string()));

        let by_sno = catalog
            .resolve_sno("1341731")
            .resolved()
            .unwrap_or_else(|| panic!("SNO must resolve"));
        assert_eq!(by_sno.id_name, by_alias.id_name);

        let by_id_name = catalog
            .resolve_id_name(&by_alias.id_name)
            .resolved()
            .unwrap_or_else(|| panic!("exact IdName must resolve"));
        assert_eq!(by_id_name.id_name, by_alias.id_name);
        assert_eq!(
            catalog
                .resolve("AttackSpeed")
                .resolved()
                .map(|row| row.id_name.clone()),
            Some(by_alias.id_name.clone())
        );

        assert!(catalog.resolve("NoSuchAffixKey").is_unknown());
        assert!(catalog.first_match("NoSuchAffixKey").is_none());
    }

    /// The pinned catalog state resolves every affix alias and SNO to exactly
    /// one identity, so the upstream `FirstOrDefault` provider lookups never
    /// have to guess. A catalog revision that breaks this shows up here as a
    /// visible migration finding instead of a silently picked row.
    #[test]
    fn pinned_affix_catalog_lookup_keys_are_unambiguous() {
        let repo = AssetRepository::new(workspace_root());
        let catalog =
            IdentityCatalog::affixes(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        let mut keys = 0usize;
        for row in catalog.rows() {
            assert!(
                !row.aliases.is_empty() && row.aliases.len() == row.snos.len(),
                "pinned affix row {} carries parallel alias/SNO lists",
                row.id_name
            );
            for key in row.aliases.iter().chain(&row.snos) {
                keys += 1;
                match catalog.resolve(key) {
                    IdentityResolution::Resolved(resolved) => {
                        assert_eq!(
                            resolved.id_name, row.id_name,
                            "key {key} resolved to a different identity"
                        );
                    }
                    IdentityResolution::Ambiguous(candidates) => panic!(
                        "key {key} is ambiguous: {:?}",
                        candidates
                            .iter()
                            .map(|candidate| candidate.id_name.as_str())
                            .collect::<Vec<_>>()
                    ),
                    IdentityResolution::Unknown => panic!("key {key} did not resolve"),
                }
            }
        }
        assert_eq!(
            keys, 8510,
            "pinned state carries 4255 aliases and 4255 SNOs"
        );

        for id_name in catalog.rows().iter().map(|row| row.id_name.clone()) {
            assert_eq!(
                catalog
                    .resolve_id_name(&id_name)
                    .resolved()
                    .map(|row| row.id_name.clone()),
                Some(id_name)
            );
        }
    }

    #[test]
    fn item_type_catalog_separates_canonical_id_from_display_label() {
        let repo = AssetRepository::new(workspace_root());
        let catalog =
            ItemTypeCatalog::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));

        assert!(catalog.is_canonical("helm"));
        assert!(!catalog.is_canonical("Legendary Helm"));
        assert_eq!(
            catalog.display_label("helm", "Legendary"),
            Some("Legendary Helm")
        );
        assert_eq!(
            catalog.resolve("Legendary Helm").resolved(),
            Some("helm"),
            "a display label resolves onto the canonical id, never the other way round"
        );
        assert_eq!(catalog.resolve("helm").resolved(), Some("helm"));
        assert!(catalog.resolve("No Such Item Type").is_unknown());
        assert_eq!(catalog.type_ids().len(), 18);
    }

    fn workspace_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        match manifest_dir.parent().and_then(|crates| crates.parent()) {
            Some(root) => root.to_path_buf(),
            None => panic!("could not resolve workspace root"),
        }
    }
}
