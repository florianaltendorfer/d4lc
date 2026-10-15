use super::build_import::MultiBuildRow;
use super::overlay_projection::{overlay_projection_source, project_overlay_marker_mode};
use super::AppState;
use d4lc_core::Rect;
use d4lc_rules::{
    AffixPreset, AffixRowText, AffixRuleRow, AffixValueFilter, AffixValueThreshold, NonAffixPreset,
    NonAffixRowVerdict, ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules,
    ParsedNonAffixForRules, RuleAffixMatch, RuleIdOverlayLocation, RuleMatchTrace,
    RuleNonAffixKind, RuleOverlayMarkerMode, RulePowerKind, RulePowerMatch, RuleSourceNote,
    RuleTradeMarker, SigilDisplayMode, TradeAffixRequirement, TradeItemRule,
};

/// Map a parsed affix row's flags onto the upstream affix kind. The
/// greater flag wins first, exactly like the row mapping the live trade
/// lookup has always used.
fn parsed_affix_kind(is_greater: bool, is_implicit: bool, is_tempered: bool) -> ParsedAffixKind {
    if is_greater {
        ParsedAffixKind::Greater
    } else if is_implicit {
        ParsedAffixKind::Implicit
    } else if is_tempered {
        ParsedAffixKind::Tempered
    } else {
        ParsedAffixKind::Normal
    }
}

/// The current item snapshot's parsed affix rows as per-row rule inputs
/// (`d4lc_rules::AffixRuleRow`, the upstream `GetAffix` per-row shape),
/// paired with the position of the view each row was built from.
///
/// Mapping: rows the parser could not resolve to an affix id are
/// skipped (upstream only draws markers for rows that converted into
/// `ItemAffixes`). A row carries its tooltip-wide `row_id` only when the
/// transported OCR row geometry has an entry for it — rows without geometry
/// keep `None` and can therefore never anchor a marker. The segment-local
/// line index is never the key: it restarts in every OCR segment.
fn affix_rule_rows_from_current_item(state: &AppState) -> Vec<(usize, AffixRuleRow)> {
    let current = state.current_item();
    current
        .affixes
        .iter()
        .enumerate()
        .filter_map(|(index, affix)| {
            let affix_id = affix.affix_id.as_deref()?;
            Some((
                index,
                AffixRuleRow {
                    affix_id: affix_id.to_string(),
                    item_type: current.item_type.clone().unwrap_or_default(),
                    kind: parsed_affix_kind(affix.is_greater, affix.is_implicit, affix.is_tempered),
                    row_id: current
                        .affix_row_geometry
                        .iter()
                        .any(|geometry| geometry.row_id == affix.row_id)
                        .then_some(affix.row_id),
                    value_text: affix.value_text.clone(),
                },
            ))
        })
        .collect()
}

/// Evaluate the current item's parsed affix rows against the selected
/// preset (upstream `AffixManager.GetAffix` per detected row,
/// AffixManager.cs:530-560) and map the traces onto the row entries the
/// overlay draws.
///
/// One entry per evaluated item row: a matched row keeps the matched preset
/// entry's color and shape flags, an unmatched row carries the upstream red
/// default so the `HideUnwanted` marker mode filters it in the descriptor
/// builder. A selected preset that does not exist leaves every row on the
/// red default like upstream's `preset == null` branch
/// (AffixManager.cs:540-541). Preset entries absent from the item produce no
/// entry (they draw nothing upstream). Returns the entries, their traces and
/// the number of rows that matched.
pub(super) fn project_rule_affix_row_matches(
    state: &AppState,
    selected_preset: &str,
    presets: &[AffixPreset],
) -> (Vec<RuleAffixMatch>, Vec<RuleMatchTrace>, usize) {
    let current = state.current_item();
    let source = overlay_projection_source("current item affix row evaluation");
    let rows = affix_rule_rows_from_current_item(state);
    let thresholds = minimal_value_thresholds(state);
    let traces = d4lc_rules::trace_affix_rows(
        &rows.iter().map(|(_, row)| row.clone()).collect::<Vec<_>>(),
        selected_preset,
        presets,
        minimal_value_filter(state, &thresholds),
    );
    let matched_count = traces.iter().filter(|trace| trace.is_match()).count();
    let matches = rows
        .iter()
        .zip(&traces)
        .map(|((view_index, _), trace)| {
            RuleAffixMatch::from_trace(
                trace,
                affix_row_text(&current.affixes[*view_index]),
                0,
                source.clone(),
            )
        })
        .collect();

    (matches, traces, matched_count)
}

/// The display text an affix row's marker entry carries.
fn affix_row_text(view: &d4lc_gui::CurrentItemAffixView) -> AffixRowText<'_> {
    AffixRowText {
        label: Some(view.label.as_str()),
        value_text: view.value_text.as_deref(),
        source_text: view.source_text.as_str(),
    }
}

/// The persisted per-affix minimal values as the rules' threshold list
/// (upstream `AffixManager.GetAffixMinimalValue`,
/// AffixManager.cs:610-613 — a missing id defaults to `0`).
fn minimal_value_thresholds(state: &AppState) -> Vec<AffixValueThreshold> {
    state
        .minimal_affix_values
        .iter()
        .map(|(affix_id, threshold)| AffixValueThreshold {
            affix_id: affix_id.clone(),
            threshold: *threshold,
        })
        .collect()
}

/// The minimal-value filter of the rules snapshot
/// (`IsMinimalAffixValueFilterEnabled` plus the stored thresholds).
fn minimal_value_filter<'a>(
    state: &AppState,
    thresholds: &'a [AffixValueThreshold],
) -> AffixValueFilter<'a> {
    AffixValueFilter {
        enabled: state.shell.settings.minimal_affix_value_filter_enabled,
        thresholds,
    }
}

/// The current item snapshot's classified non-affix rows as per-row rule
/// inputs (`d4lc_rules::ParsedNonAffixForRules`), paired with the position
/// of the view row each row was built from.
///
/// Mapping, mirroring `affix_rule_rows_from_current_item`: rows the
/// parser could not resolve to a canonical id are skipped (upstream only
/// evaluates rows that converted to `ItemAffixes`). The kind selects the
/// upstream lookup, and the row's item type follows the upstream callers:
/// aspects and uniques compare against the item's parsed type
/// (`AffixManager.cs:632`/`:787`), runes and sigils are their own type
/// families (`:836`/`:726`).
fn non_affix_rule_rows_from_current_item(state: &AppState) -> Vec<(usize, ParsedNonAffixForRules)> {
    let current = state.current_item();
    current
        .non_affix_rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let id = row.id.as_deref()?;
            let kind = match row.kind {
                d4lc_gui::CurrentItemMatchKind::Aspect => RuleNonAffixKind::Aspect,
                d4lc_gui::CurrentItemMatchKind::Unique => RuleNonAffixKind::Unique,
                d4lc_gui::CurrentItemMatchKind::Rune => RuleNonAffixKind::Rune,
                d4lc_gui::CurrentItemMatchKind::Sigil => RuleNonAffixKind::Sigil,
            };
            Some((
                index,
                ParsedNonAffixForRules {
                    id: id.to_string(),
                    item_type: match kind {
                        RuleNonAffixKind::Rune => "rune".to_string(),
                        RuleNonAffixKind::Sigil => "sigil".to_string(),
                        _ => current.item_type.clone().unwrap_or_default(),
                    },
                    kind,
                    is_unique_item: row.kind == d4lc_gui::CurrentItemMatchKind::Unique,
                },
            ))
        })
        .collect()
}

/// The settings sigil display mode in the frozen rules shape — upstream
/// `GetSigil` colors non-listed sigils Red while whitelisting and Green
/// while blacklisting (`AffixManager.cs:716-721`).
fn sigil_display_mode(state: &AppState) -> SigilDisplayMode {
    match state.shell.settings.sigil_display_mode {
        d4lc_config::SigilDisplayModeSetting::Whitelisting => SigilDisplayMode::Whitelisting,
        d4lc_config::SigilDisplayModeSetting::Blacklisting => SigilDisplayMode::Blacklisting,
    }
}

/// The non-affix half of the row projection: verdicts mapped onto the
/// frozen power/rune/sigil descriptor inputs plus the row accounting the
/// projection's wanted/unwanted decision uses.
///
/// Hide Unwanted mirrors the frozen affix descriptor filter
/// (`push_affix_overlay_descriptors` skips Red): a Red row's marker
/// location is withheld while its verdict stays in the result, so the
/// frozen builder draws exactly the non-Red markers and the projection
/// keeps its explanation.
#[derive(Default)]
pub(super) struct NonAffixMarkerProjection {
    pub(super) matched_power: Option<RulePowerMatch>,
    pub(super) power_location: Option<RuleIdOverlayLocation>,
    /// Per-build power matches for the multi-build draw (upstream
    /// `ItemAspectBuildList`, OH:487-522): one entry per build column that
    /// matched an aspect/unique row, in column order.
    pub(super) matched_powers: Vec<RulePowerMatch>,
    /// Per-build power location copies, already shifted by the build's
    /// column offset (`5 - (i * 20)`, OH:337); pairs with
    /// `matched_powers` in order.
    pub(super) power_build_locations: Vec<RuleIdOverlayLocation>,
    pub(super) matched_runes: Vec<String>,
    pub(super) rune_locations: Vec<RuleIdOverlayLocation>,
    pub(super) matched_sigil: Option<String>,
    pub(super) sigil_location: Option<RuleIdOverlayLocation>,
    /// Per-build sigil matches plus their pre-shifted location copies for
    /// the multi-build draw (upstream draws each build's dungeon square in
    /// its own column, OH:331-419); pairs in order.
    pub(super) matched_sigils: Vec<String>,
    pub(super) sigil_build_locations: Vec<RuleIdOverlayLocation>,
    pub(super) evaluated_rows: usize,
    pub(super) matched_rows: usize,
}

impl NonAffixMarkerProjection {
    /// Whether any non-affix marker input was projected.
    pub(super) fn has_markers(&self) -> bool {
        self.matched_power.is_some()
            || !self.matched_powers.is_empty()
            || !self.matched_runes.is_empty()
            || self.matched_sigil.is_some()
            || !self.matched_sigils.is_empty()
    }

    /// Record one evaluated non-affix row's verdict. The verdict always
    /// lands in the result — mirroring the affix wiring, where unmatched
    /// red rows stay in `matched_affixes` and the frozen builder applies
    /// Hide Unwanted — while the marker location is only anchored when
    /// `draw` allows it (the row survived Hide Unwanted's red rule) and
    /// carries a marker anchor. The single-slot kinds keep their first
    /// claim (upstream keeps ONE aspect and ONE sigil per tooltip).
    /// `tier_text` is the only marker text: the dungeon square draws the
    /// tier (OverlayHandler.cs:284-292), aspect, unique and rune markers
    /// draw no text at all (OverlayHandler.cs:463-485, :316-319).
    fn record_row_verdict(
        &mut self,
        row: &ParsedNonAffixForRules,
        view: &d4lc_gui::CurrentItemNonAffixRowView,
        color: String,
        source: &RuleSourceNote,
        draw: bool,
        tier_text: Option<String>,
    ) {
        let text = tier_text;
        // The marker sits at the row's marker anchor (tooltip left edge,
        // the aspect/rune/sigil marker row, OverlayHandler.cs:263-264,
        // :471-475); a row without one has no upstream position.
        let location = (draw.then_some(view.marker_anchor))
            .flatten()
            .map(|geometry| RuleIdOverlayLocation {
                id: row.id.clone(),
                geometry,
                color,
                text,
            });
        match row.kind {
            RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => {
                if self.matched_power.is_some() {
                    return;
                }
                self.matched_power = Some(RulePowerMatch {
                    power_id: row.id.clone(),
                    power_kind: match row.kind {
                        RuleNonAffixKind::Unique => RulePowerKind::Unique,
                        _ => RulePowerKind::Aspect,
                    },
                    source: source.clone(),
                });
                self.power_location = location;
            }
            RuleNonAffixKind::Rune => {
                self.matched_runes.push(row.id.clone());
                if let Some(location) = location {
                    self.rune_locations.push(location);
                }
            }
            RuleNonAffixKind::Sigil => {
                if self.matched_sigil.is_some() {
                    return;
                }
                self.matched_sigil = Some(row.id.clone());
                self.sigil_location = location;
            }
        }
    }

    /// Record one matched non-affix row for the multi-build draw: the
    /// first claim across builds still mirrors into the single slots
    /// (upstream keeps ONE `ItemAspectLocation` per tooltip and the
    /// current-item views read the single slots), while the per-build
    /// copies carry this build's column offset (`5 - (i * 20)`, OH:337)
    /// so the frozen builder draws one aspect/sigil marker per matching
    /// build column (`DrawGraphicsAspectsMulti`, OH:487-522; dungeon
    /// squares via `DrawGraphicsAffixesMulti`, OH:331-419). Runes keep
    /// one shared marker per id.
    fn record_multi_build_row_verdict(
        &mut self,
        row: &ParsedNonAffixForRules,
        view: &d4lc_gui::CurrentItemNonAffixRowView,
        color: String,
        source: &RuleSourceNote,
        column_offset: i32,
        tier_text: Option<String>,
    ) {
        self.record_row_verdict(row, view, color.clone(), source, true, tier_text.clone());
        // Like the single slots, a per-build marker needs the row's marker
        // anchor: without it nothing can anchor.
        let Some(rect) = view.marker_anchor else {
            return;
        };
        let text = tier_text;
        let location = RuleIdOverlayLocation {
            id: row.id.clone(),
            geometry: Rect {
                x: rect.x + column_offset,
                ..rect
            },
            color,
            text,
        };
        match row.kind {
            RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => {
                self.matched_powers.push(RulePowerMatch {
                    power_id: row.id.clone(),
                    power_kind: match row.kind {
                        RuleNonAffixKind::Unique => RulePowerKind::Unique,
                        _ => RulePowerKind::Aspect,
                    },
                    source: source.clone(),
                });
                self.power_build_locations.push(location);
            }
            RuleNonAffixKind::Rune => {}
            RuleNonAffixKind::Sigil => {
                self.matched_sigils.push(row.id.clone());
                self.sigil_build_locations.push(location);
            }
        }
    }
}

/// The dungeon tier text for a sigil row: only while the persisted
/// `dungeon_tiers_enabled` setting is on and the sigil's game-data type is
/// `Dungeon` (upstream gates the tier text on the sigil type,
/// `AffixManager.GetSigilType` in OverlayHandler.cs:279-293), the stored
/// `Config/DungeonTiers.json` tier or the upstream `F` default
/// (`AffixManager.GetSigilDungeonTier`, AffixManager.cs:744-747) — the same
/// projection the Sigil grid's tier badge applies
/// (`actions::affix::sigil_tier_label`). A non-dungeon sigil draws no tier
/// text, and an id the loaded rules catalog does not carry is not a dungeon
/// sigil (upstream's type lookup yields `""` for it).
fn sigil_dungeon_tier_text(state: &AppState, row: &ParsedNonAffixForRules) -> Option<String> {
    if row.kind != RuleNonAffixKind::Sigil || !state.shell.settings.dungeon_tiers_enabled {
        return None;
    }
    if sigil_type(state, &row.id).as_deref() != Some("Dungeon") {
        return None;
    }
    Some(
        state
            .dungeon_tiers
            .get(&row.id)
            .cloned()
            .unwrap_or_else(|| d4lc_gui::affix::SIGIL_DUNGEON_TIER_DEFAULT.to_string()),
    )
}

/// The game-data `Type` token of a sigil id from the loaded rules catalog
/// (upstream `AffixManager.GetSigilType`, AffixManager.cs:749-760, reads the
/// same token from the Sigils data; an id the data does not know yields `""`
/// upstream, `None` here).
fn sigil_type(state: &AppState, sigil_id: &str) -> Option<String> {
    state
        .affix_catalog
        .as_ref()
        .and_then(|catalog| catalog.entry(d4lc_rules::CatalogKind::Sigil, sigil_id))
        .and_then(|entry| entry.sigil_type.clone())
}

/// Evaluate the current item's classified non-affix rows (aspect, unique,
/// rune, sigil — the transported `CurrentItemSnapshot.non_affix_rows`)
/// against the selected preset's non-affix lists and map the verdicts onto
/// the marker slots the overlay draws.
///
/// Per-row mirror of the dedicated upstream lookups via
/// `d4lc_rules::evaluate_non_affix_preset_rows`: a matched row draws in the
/// matched preset entry's color, an unmatched row draws the red default —
/// except sigils under the blacklisting display mode, whose default is the
/// upstream green (`AffixManager.cs:716-721`).
pub(super) fn project_rule_non_affix_matches(
    state: &AppState,
    preset: &NonAffixPreset,
) -> NonAffixMarkerProjection {
    let current = state.current_item();
    let source = overlay_projection_source("current item non-affix row evaluation");
    let rows = non_affix_rule_rows_from_current_item(state);
    let mut projection = NonAffixMarkerProjection {
        evaluated_rows: rows.len(),
        ..NonAffixMarkerProjection::default()
    };
    if rows.is_empty() {
        return projection;
    }
    let hide_unwanted = project_overlay_marker_mode(state.shell.settings.overlay_marker_mode)
        == RuleOverlayMarkerMode::HideUnwanted;
    let verdicts = d4lc_rules::evaluate_non_affix_preset_rows(
        &rows.iter().map(|(_, row)| row.clone()).collect::<Vec<_>>(),
        preset,
        sigil_display_mode(state),
    );

    for ((view_index, row), verdict) in rows.iter().zip(verdicts) {
        let color = match verdict {
            NonAffixRowVerdict::Matched { color, .. } => {
                projection.matched_rows += 1;
                color
            }
            NonAffixRowVerdict::Unmatched { color, .. } => color,
        };
        let draw = !(hide_unwanted && color == "Red");
        projection.record_row_verdict(
            row,
            &current.non_affix_rows[*view_index],
            color,
            &source,
            draw,
            sigil_dungeon_tier_text(state, row),
        );
    }

    projection
}

/// Upstream `DrawGraphicsAffixesMulti` per-build column offset
/// (OverlayHandler.cs:337): build i's whole marker column shifts its
/// `left` by `5 - (i * 20)`.
const MULTI_BUILD_FIRST_COLUMN_OFFSET: i32 = 5;
const MULTI_BUILD_COLUMN_STRIDE: i32 = 20;

/// Multi-build per-build evaluation (upstream
/// `ScreenProcessHandler.SetMultiBuildMode`, ScreenProcessHandler.cs:1748-1852):
/// every configured build's preset re-evaluates the same detected rows and a
/// matched row draws in that build's color. The configured `MultiBuildRow`
/// name IS the preset name (upstream `MultiBuild.Name` selects
/// `AffixPresets.FirstOrDefault(preset => preset.Name.Equals(buildName))`);
/// a build whose preset is missing draws nothing, exactly like upstream's
/// early return (the name is validated with a warning when the multi-build
/// config is confirmed, mirroring upstream `ValidateMultiBuild`).
///
/// Rows no configured build lists draw nothing: upstream
/// `DrawGraphicsAffixesMulti`/`DrawGraphicsAspectsMulti` hide the Red
/// default in every multi column, so unmatched rows produce no marker at
/// all — regardless of the marker mode.
pub(super) fn project_rule_multi_build_row_matches(
    state: &AppState,
    rule_presets: &d4lc_gui::RulePresetProjection,
) -> (
    Vec<RuleAffixMatch>,
    Vec<RuleMatchTrace>,
    NonAffixMarkerProjection,
    usize,
    usize,
) {
    let source = overlay_projection_source("multi-build per-build row evaluation");
    let current = state.current_item();
    let affix_rows = affix_rule_rows_from_current_item(state);
    let non_affix_rows = non_affix_rule_rows_from_current_item(state);
    let mut builds: Vec<&MultiBuildRow> = state
        .build_import
        .multi_builds
        .iter()
        .filter(|build| !build.name.trim().is_empty())
        .collect();
    // Upstream iterates `MultiBuildList` in stored order; the row index is
    // the column order the overlay draws the builds in.
    builds.sort_by_key(|build| build.index);

    let mut matches = Vec::new();
    let mut traces = Vec::new();
    let thresholds = minimal_value_thresholds(state);
    let filter = minimal_value_filter(state, &thresholds);
    let mut projection = NonAffixMarkerProjection::default();
    let mut affix_row_matched = vec![false; affix_rows.len()];
    let mut non_affix_row_matched = vec![false; non_affix_rows.len()];
    let mut affix_rows_evaluated = false;
    let mut non_affix_rows_evaluated = false;

    for (column, build) in builds.iter().enumerate() {
        let build_column_offset =
            MULTI_BUILD_FIRST_COLUMN_OFFSET - (column as i32 * MULTI_BUILD_COLUMN_STRIDE);
        // Aspect/unique and sigil markers draw per build column (upstream
        // `DrawGraphicsAspectsMulti` OH:487-522 and the multi dungeon
        // squares OH:331-419), so each build claims its own first match;
        // runes stay one shared marker per id.
        let mut power_claimed_for_build = false;
        let mut sigil_claimed_for_build = false;
        let affix_preset = rule_presets
            .affix_presets
            .iter()
            .find(|preset| preset.name == build.name);
        if let Some(affix_preset) = affix_preset {
            if !affix_rows_evaluated {
                projection.evaluated_rows += affix_rows.len();
                affix_rows_evaluated = true;
            }
            let build_traces = affix_rows.iter().map(|(_, row)| {
                d4lc_rules::trace_affix_row(row, &build.name, Some(affix_preset), filter)
                    .for_build(column, &build.color)
            });
            for ((view_index, _), trace) in affix_rows.iter().zip(build_traces) {
                if trace.is_match() {
                    affix_row_matched[*view_index] = true;
                    // Every multi-build column hides red markers, whatever
                    // the marker mode (OverlayHandler.cs:350): an unmatched
                    // row and a build colored red draw nothing.
                    if !d4lc_rules::is_upstream_red(&trace.color) {
                        matches.push(RuleAffixMatch::from_trace(
                            &trace,
                            affix_row_text(&current.affixes[*view_index]),
                            build_column_offset,
                            source.clone(),
                        ));
                    }
                }
                traces.push(trace);
            }
        }

        let non_affix_preset = rule_presets
            .non_affix_presets
            .iter()
            .find(|preset| preset.name == build.name);
        let Some(non_affix_preset) = non_affix_preset else {
            continue;
        };
        if !non_affix_rows_evaluated {
            projection.evaluated_rows += non_affix_rows.len();
            non_affix_rows_evaluated = true;
        }
        let verdicts = d4lc_rules::evaluate_non_affix_preset_rows(
            &non_affix_rows
                .iter()
                .map(|(_, row)| row.clone())
                .collect::<Vec<_>>(),
            non_affix_preset,
            sigil_display_mode(state),
        );
        for ((view_index, row), verdict) in non_affix_rows.iter().zip(verdicts) {
            let NonAffixRowVerdict::Matched { .. } = verdict else {
                continue;
            };
            non_affix_row_matched[*view_index] = true;
            // Upstream's multi draw paths hide Red markers in every
            // column, so a row whose winning build color is Red draws
            // nothing either (a later build may still claim the slot).
            if build.color == "Red" {
                continue;
            }
            // Aspect/unique and sigil markers draw per build at this
            // build's own column (first matching row wins the column);
            // runes stay one shared marker per id, so the first build
            // (column order) that lists the rune owns it and later
            // builds only keep the accounting.
            let already_claimed = match row.kind {
                RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => power_claimed_for_build,
                RuleNonAffixKind::Rune => projection
                    .matched_runes
                    .iter()
                    .any(|rune_id| rune_id == &row.id),
                RuleNonAffixKind::Sigil => sigil_claimed_for_build,
            };
            if already_claimed {
                continue;
            }
            match row.kind {
                RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => {
                    power_claimed_for_build = true
                }
                RuleNonAffixKind::Sigil => sigil_claimed_for_build = true,
                RuleNonAffixKind::Rune => {}
            }
            projection.record_multi_build_row_verdict(
                row,
                &current.non_affix_rows[*view_index],
                build.color.clone(),
                &source,
                build_column_offset,
                sigil_dungeon_tier_text(state, row),
            );
        }
    }

    let matched_rows = affix_row_matched.iter().filter(|matched| **matched).count()
        + non_affix_row_matched
            .iter()
            .filter(|matched| **matched)
            .count();
    let evaluated_rows = projection.evaluated_rows;
    (matches, traces, projection, matched_rows, evaluated_rows)
}

pub(super) fn project_rule_trade_marker(state: &AppState) -> Option<RuleTradeMarker> {
    if let Some(marker) = state.trade_imports().current_trade_marker.as_ref() {
        return Some(RuleTradeMarker {
            trade_item_id: marker.trade_item_id.clone(),
            value_text: marker.value_text.clone(),
            source: overlay_projection_source("current trade marker projection"),
        });
    }

    // Live lookup (upstream ScreenProcessHandler.cs:489-492): once the
    // tooltip carries parsed affixes, the marker derives exclusively
    // from `TradeItemManager.FindTradeItem` over the user's wanted
    // rows — a parsed item that matches no wanted item shows no
    // marker.
    if live_parsed_item_available(state) {
        return live_lookup_trade_marker(state);
    }

    // No parsed tooltip (the production steady state between
    // captures): the projection surfaces the selected (or first)
    // wanted row so the trade marker keeps pointing at the configured
    // trade rows.
    let trade_item =
        selected_trade_item_row(state).or_else(|| state.trade_imports().trade_items.first())?;
    Some(RuleTradeMarker {
        trade_item_id: trade_item.trade_item_id.clone(),
        value_text: trade_item.value_text.clone(),
        source: overlay_projection_source("selected trade item projection"),
    })
}

/// A parsed tooltip the trade lookup can run on: at least one parsed
/// affix (upstream gates `FindTradeItem` on
/// `_currentTooltip.ItemAffixes.Any()`). The item type participates in
/// the comparison exactly as parsed — an empty string only matches
/// wanted rows without a type — so a partially parsed tooltip (affixes
/// read, type line lost) still runs the real lookup.
fn live_parsed_item_available(state: &AppState) -> bool {
    !state.current_item().affixes.is_empty()
}

/// Run the live trade lookup for the parsed current item over the
/// user's wanted rows: `d4lc_rules::find_trade_marker` is the match
/// core of `apply_trade_item_lookup`
/// (`TradeItemManager.FindTradeItem`).
fn live_lookup_trade_marker(state: &AppState) -> Option<RuleTradeMarker> {
    let current = state.current_item();
    let item = ParsedItemForRules {
        item_type: current.item_type.clone().unwrap_or_default(),
        rarity: current.rarity.clone(),
        affixes: current
            .affixes
            .iter()
            .filter_map(|affix| {
                Some(ParsedAffixForRules {
                    affix_id: affix.affix_id.as_deref()?.to_string(),
                    label: Some(affix.label.clone()),
                    value_text: affix.value_text.clone(),
                    source_text: affix.source_text.clone(),
                    row_id: Some(affix.row_id),
                    kind: parsed_affix_kind(affix.is_greater, affix.is_implicit, affix.is_tempered),
                })
            })
            .collect(),
    };
    let trade_items: Vec<TradeItemRule> = state
        .trade_imports()
        .trade_items
        .iter()
        .map(|row| TradeItemRule {
            trade_item_id: row.trade_item_id.clone(),
            item_type: row.item_type.clone(),
            value_text: row.value_text.clone(),
            affixes: row
                .affixes
                .iter()
                .map(|affix| TradeAffixRequirement {
                    affix_id: affix.affix_id.clone(),
                    is_greater: affix.is_greater,
                })
                .collect(),
        })
        .collect();
    d4lc_rules::find_trade_marker(&item, &trade_items)
}

pub(super) fn selected_trade_item_row(state: &AppState) -> Option<&d4lc_gui::TradeItemRow> {
    let selected_id = state.trade_imports().selected_trade_item_id.as_deref()?;
    state
        .trade_imports()
        .trade_items
        .iter()
        .find(|trade_item| trade_item.trade_item_id == selected_id)
}

pub(super) fn first_selected_affix_item_type(state: &AppState) -> Option<String> {
    state
        .affix_preset_editor
        .selected_preset_entries()
        .unwrap_or(&[])
        .iter()
        .find_map(|entry| entry.item_type.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
