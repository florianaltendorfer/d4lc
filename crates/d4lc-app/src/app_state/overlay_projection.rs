use super::rule_rows::{
    first_selected_affix_item_type, project_rule_affix_row_matches,
    project_rule_multi_build_row_matches, project_rule_non_affix_matches,
    project_rule_trade_marker, selected_trade_item_row,
};
use super::AppState;
use d4lc_rules::{
    ImportedParagonRoute, NonAffixPreset, ParagonOverlayLayout, ParagonOverlayMode,
    ParagonOverlayState, RuleAffixOverlayLocation, RuleBranchExplanation, RuleCheck, RuleCheckKind,
    RuleDecision, RuleEvaluationResult, RuleExplanation, RuleOverlayLayout, RuleOverlayMarkerMode,
    RuleOverlaySettings, RuleSourceNote, RuleTradeMarker,
};

// ---------------------------------------------------------------------------
// OverlayDescriptorState — mirrors the CLI's `LiveOverlayDescriptorState`
// but lives in app-owned space.
// ---------------------------------------------------------------------------

/// Complete overlay descriptor state owned by the desktop app.
///
/// When `Some`, the live loop uses `d4lc_rules::build_live_overlay_descriptors(...)`
/// to produce `Vec<RuleOverlayDescriptor>` per frame.  When `None`, no descriptors
/// are generated (pre-startup or post-shutdown).
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayDescriptorState {
    /// Rule overlay state (affix highlights, power markers, trade markers).
    pub rule: Option<RuleOverlayState>,
    /// Paragon overlay state (board navigation, node highlights).
    pub paragon: Option<ParagonOverlayStateBundle>,
    /// `IsParagonModeActive`: upstream draws the paragon overlay *instead of*
    /// the affix, aspect and trade markers (`if` / `else`,
    /// OverlayHandler.cs:154-163), so while it is set the rule state feeds
    /// the traces but draws no marker. Independent of whether the paragon
    /// board itself could be built.
    pub paragon_mode_active: bool,
}

/// Rule overlay inputs needed by `d4lc_rules::build_overlay_descriptors(...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleOverlayState {
    pub result: RuleEvaluationResult,
    pub settings: RuleOverlaySettings,
    pub layout: RuleOverlayLayout,
}

/// Paragon overlay inputs needed by `d4lc_rules::build_paragon_overlay_descriptors(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagonOverlayStateBundle {
    pub state: ParagonOverlayState,
    pub layout: ParagonOverlayLayout,
}

impl AppState {
    /// Rebuild the app-owned overlay descriptor projection from the current
    /// persisted/app-shell state.
    pub fn rebuild_overlay_descriptor_state(&mut self) {
        self.refresh_overlay_window_geometry();
        self.overlay_descriptor_state = project_overlay_descriptor_state(self);
        self.overlay_descriptor_state_from_app = true;
    }

    /// Resolve the real game-window geometry once per live session so the
    /// paragon board layout uses the window the overlay surface actually
    /// covers (upstream draws the board centered on `_window` per draw,
    /// OH:673-674). The resolved capture engine decides the geometry
    /// source: on the Wayland portal engine there is no X11 window to
    /// query — the stream rect the live loop published with the engine
    /// resolution is used instead (output-anchored positioning for
    /// multi-monitor streams belongs to the Wayland overlay). The
    /// X11 engine keeps the same discovery the overlay start performs; a
    /// failure keeps `None` — no invented geometry — and the next rebuild
    /// retries while paragon mode is on, so a game window that appears
    /// later is still picked up. Without the real-runtime feature there is
    /// no window discovery and the field stays `None`: descriptor tests
    /// set it directly.
    fn refresh_overlay_window_geometry(&mut self) {
        #[cfg(feature = "real-runtime")]
        if self.shell.settings.paragon_mode_active && self.overlay_window_geometry.is_none() {
            if let Some(geometry) = self
                .capture_engine
                .as_ref()
                .and_then(|engine| engine.wayland_stream_geometry)
            {
                // Wayland portal engine: the published stream rect, never
                // an X11 query.
                self.overlay_window_geometry = Some(geometry);
            } else if let Ok(reference) = crate::runtime_prepare::resolve_capture_source(None, None)
            {
                self.overlay_window_geometry = Some(reference.geometry);
            }
        }
        #[cfg(not(feature = "real-runtime"))]
        {
            let _ = self;
        }
        #[cfg(feature = "real-runtime")]
        {
            self.bump_rules_input_revision();
        }
    }
}

// ---------------------------------------------------------------------------
// OverlayDescriptorState helpers
// ---------------------------------------------------------------------------

impl OverlayDescriptorState {
    /// Build a fresh `OverlayDescriptorState` from rule evaluation result,
    /// settings, layout, and optional paragon state.
    #[cfg(test)]
    pub fn new(
        rule: Option<(RuleEvaluationResult, RuleOverlaySettings, RuleOverlayLayout)>,
        paragon: Option<(ParagonOverlayState, ParagonOverlayLayout)>,
    ) -> Self {
        Self {
            rule: rule.map(|(result, settings, layout)| RuleOverlayState {
                result,
                settings,
                layout,
            }),
            paragon_mode_active: paragon.is_some(),
            paragon: paragon.map(|(state, layout)| ParagonOverlayStateBundle { state, layout }),
        }
    }

    /// True when at least rule or paragon state is present.
    pub fn has_any_state(&self) -> bool {
        self.rule.is_some() || self.paragon.is_some()
    }
}

/// The rule projection of `state` — the evaluation entry point of the live
/// loop's immutable rules snapshot (`runtime_bridge::live_state`). Pure: it
/// reads `state` and resolves nothing from the environment.
pub(crate) fn evaluate_overlay_descriptor_state(
    state: &AppState,
) -> Option<OverlayDescriptorState> {
    project_overlay_descriptor_state(state)
}

fn project_overlay_descriptor_state(state: &AppState) -> Option<OverlayDescriptorState> {
    let projected = OverlayDescriptorState {
        rule: project_rule_overlay_state(state),
        paragon: project_paragon_overlay_state(state),
        paragon_mode_active: state.shell.settings.paragon_mode_active,
    };
    projected.has_any_state().then_some(projected)
}

fn project_rule_overlay_state(state: &AppState) -> Option<RuleOverlayState> {
    // Upstream overlay on/off gate (OverlayHandler.cs:133): while the
    // overlay is off, the draw pass clears the current tooltip every
    // frame, so no affix, aspect, rune, sigil, or trade marker can
    // anchor — the affix screen's overlay power switch and the menu
    // lock's "Turn overlay off" both end in `ToggleOverlayMessage`
    // (OH:1117-1140). The persisted `overlay_enabled` setting is d4lc's
    // power switch; here it gates the whole rule-marker projection at
    // this one seam. Paragon projects separately (`IsParagonModeActive`
    // draws regardless of the overlay lock upstream, OH:154-164).
    if !state.shell.settings.overlay_enabled {
        return None;
    }

    let selected_preset_name = state.affix_preset_editor.selected_preset_name.clone();
    // Real per-frame evaluation inputs: the presets converted into the
    // upstream rules shape (the gui `to_rule_presets` conversion path),
    // plus the current item snapshot's parsed affix and non-affix rows.
    let rule_presets = state.affix_preset_editor.to_rule_presets();

    // Multi-build re-evaluates the same detected rows against every
    // configured build's preset (upstream `SetMultiBuildMode`); the
    // single-build path evaluates against the selected preset only.
    let (matched_affixes, affix_traces, non_affix, matched_row_count, evaluated_row_count) =
        if state.shell.settings.multi_build_mode_enabled {
            project_rule_multi_build_row_matches(state, &rule_presets)
        } else {
            let preset_name = selected_preset_name.as_deref().unwrap_or_default();
            // A missing selected preset leaves every row on its lookup's
            // default (`preset == null`, AffixManager.cs:540-541, :630,
            // :717, :785, :834), exactly like a preset without entries.
            let missing_non_affix_preset = NonAffixPreset {
                name: preset_name.to_string(),
                powers: Vec::new(),
                runes: Vec::new(),
                sigils: Vec::new(),
            };
            let selected_non_affix_preset = rule_presets
                .non_affix_presets
                .iter()
                .find(|preset| preset.name == preset_name)
                .unwrap_or(&missing_non_affix_preset);
            let (matched_affixes, affix_traces, matched_count) =
                project_rule_affix_row_matches(state, preset_name, &rule_presets.affix_presets);
            let non_affix = project_rule_non_affix_matches(state, selected_non_affix_preset);
            let matched_rows = matched_count + non_affix.matched_rows;
            let evaluated = matched_affixes.len() + non_affix.evaluated_rows;
            (
                matched_affixes,
                affix_traces,
                non_affix,
                matched_rows,
                evaluated,
            )
        };
    let trade_marker = project_rule_trade_marker(state);

    if matched_affixes.is_empty() && !non_affix.has_markers() && trade_marker.is_none() {
        return None;
    }

    // Per-row verdict semantics (upstream `AffixManager.GetAffix` per
    // detected row): an item whose rows all matched the preset is wanted;
    // evaluated rows with no match at all are unwanted. With no parsed rows
    // there is nothing to evaluate, so the projection keeps the trade/preset
    // marker decision it always surfaced.
    let any_row_evaluated = evaluated_row_count > 0;
    let final_decision = if any_row_evaluated && matched_row_count == 0 {
        RuleDecision::Unwanted
    } else {
        RuleDecision::Wanted
    };

    let item_type = state
        .current_item()
        .item_type
        .clone()
        .or_else(|| selected_trade_item_row(state).map(|item| item.item_type.clone()))
        .or_else(|| first_selected_affix_item_type(state))
        .or_else(|| {
            state
                .trade_imports()
                .import_state
                .latest_result
                .as_ref()
                .and_then(|result| result.items.first().map(|item| item.item_type.clone()))
        })
        .unwrap_or_else(|| "Unknown Item".to_string());

    let source = overlay_projection_source("app-owned persisted preset/trade projection");
    let matched_build_names = projected_build_names(state);
    let selected_build_name = matched_build_names.first().cloned();
    let branches = projected_rule_branches(selected_preset_name.as_deref(), trade_marker.as_ref());

    let mut layout = project_rule_overlay_layout(state);
    layout.power_location = non_affix.power_location.clone();
    layout.power_locations = non_affix.power_build_locations.clone();
    layout.rune_locations = non_affix.rune_locations.clone();
    layout.sigil_location = non_affix.sigil_location.clone();
    layout.sigil_locations = non_affix.sigil_build_locations.clone();

    Some(RuleOverlayState {
        result: RuleEvaluationResult {
            final_decision,
            final_source: Some(source.clone()),
            matched_affixes,
            // Per-row upstream semantics: preset entries absent from the
            // item draw nothing, so the row flow synthesizes no "missing"
            // entries (unlike the whole-item `evaluate_affix_preset` path).
            missing_affixes: Vec::new(),
            item_power_status: RuleCheck::not_evaluated(
                RuleCheckKind::ItemPowerLimit,
                source.clone(),
            ),
            minimal_value_status: RuleCheck::not_evaluated(
                RuleCheckKind::MinimalAffixValue,
                source.clone(),
            ),
            matched_power: non_affix.matched_power,
            matched_powers: non_affix.matched_powers,
            matched_runes: non_affix.matched_runes,
            matched_sigil: non_affix.matched_sigil,
            matched_sigils: non_affix.matched_sigils,
            trade_marker,
            overlay_descriptors: Vec::new(),
            affix_traces,
            explanation: RuleExplanation {
                active_preset_names: selected_preset_name.into_iter().collect(),
                matched_build_names,
                selected_build_name,
                item_type: Some(item_type),
                rarity: state.current_item().rarity.clone(),
                branches,
            },
        },
        settings: RuleOverlaySettings {
            marker_mode: project_overlay_marker_mode(state.shell.settings.overlay_marker_mode),
            is_minimal_affix_value_filter_enabled: state
                .shell
                .settings
                .minimal_affix_value_filter_enabled,
            is_trade_overlay_enabled: state.shell.settings.trade_overlay_enabled,
            item_power_limit_check_ok: item_power_limit_check_ok(state),
            // The hotkey/settings toggle gates the
            // tuning-prism hint descriptors in `d4lc-rules`.
            is_tuning_prism_hints_enabled: state.shell.settings.is_tuning_prism_hints_enabled,
        },
        layout,
    })
}

/// The imported paragon routes the overlay projects, with the selected
/// preset's name for the build panel: the most recent provider download's
/// boards (live-only; the download path creates no preset) replace the
/// selected preset's imported boards, which upstream
/// `OverlayHandler.DrawGraphicsParagon` consumes
/// (`OverlayHandler.cs:570-601`). The build panel always carries
/// `preset.Name` (`OverlayHandler.cs:608`, `:744`).
///
/// `None` when there is no selected preset to draw for, download or not:
/// upstream returns without drawing anything (`if (preset == null) return;`,
/// OverlayHandler.cs:573), where a preset without boards draws the warning
/// panel (:574-578).
fn projected_paragon_routes(state: &AppState) -> Option<(Vec<ImportedParagonRoute>, String)> {
    let selected = state.affix_preset_editor.selected_preset_name.as_deref()?;
    if !state
        .affix_preset_editor
        .presets
        .iter()
        .any(|preset| preset.name == selected)
    {
        return None;
    }
    let routes = match state.downloaded_paragon_routes.as_ref() {
        Some(routes) => routes.clone(),
        None => state
            .affix_preset_editor
            .paragon_routes_for(selected)
            .unwrap_or_default()
            .to_vec(),
    };
    Some((routes, selected.to_string()))
}

fn project_paragon_overlay_state(state: &AppState) -> Option<ParagonOverlayStateBundle> {
    if !state.shell.settings.paragon_mode_active {
        return None;
    }

    // Upstream draws the paragon board centered on the overlay window
    // and returns early without one (`DrawGraphicsParagon`'s
    // `if (_window == null) return;`, OverlayHandler.cs:570 — the
    // warning panel is equally unreachable without a window). No real
    // game-window geometry, no paragon descriptors: the previous
    // hardcoded 1920x1080 fallback drew boards at coordinates no real
    // resolution matched. `project_paragon_overlay_layout` reads the
    // resolved geometry from `overlay_window_geometry`.
    state.overlay_window_geometry?;

    // Real imported boards: localized name, rotation text, glyph and
    // the transformed 21×21 node grid straight from the import result
    // (upstream `ParagonBoard`: `Name`/`Rotation`/`Glyph`/`Nodes`).
    let (routes, build_name) = projected_paragon_routes(state)?;
    let board_count: usize = routes.iter().map(|route| route.boards.len()).sum();

    let layout = project_paragon_overlay_layout(state);
    let current_board_name = routes
        .first()
        .and_then(|route| route.boards.first())
        .map(|board| board.name.clone());
    Some(ParagonOverlayStateBundle {
        state: ParagonOverlayState {
            build_name,
            routes,
            // Initial selection; the live loop applies its persistent
            // hover navigation before building descriptors.
            current_route_index: 0,
            current_board_index: (board_count > 0).then_some(0),
            current_board_name,
            // Upstream selects the draw path per board mode
            // (OverlayHandler.cs:154-163): the collapsed-paragon-board
            // setting picks `DrawGraphicsParagonCollapsed`
            // (OH:704-839) over `DrawGraphicsParagon` (OH:568-702).
            mode: if state.shell.settings.collapsed_paragonboard_enabled {
                ParagonOverlayMode::Collapsed
            } else {
                ParagonOverlayMode::Full
            },
        },
        layout,
    })
}

fn projected_build_names(state: &AppState) -> Vec<String> {
    if let Some(result) = state.trade_imports().import_state.latest_result.as_ref() {
        if !result.source_build_name.trim().is_empty() {
            return vec![result.source_build_name.clone()];
        }
    }

    state
        .build_import
        .multi_builds
        .iter()
        .map(|build| build.name.clone())
        .collect()
}

fn projected_rule_branches(
    selected_preset_name: Option<&str>,
    trade_marker: Option<&RuleTradeMarker>,
) -> Vec<RuleBranchExplanation> {
    let mut branches = Vec::new();

    if let Some(preset_name) = selected_preset_name {
        branches.push(RuleBranchExplanation {
            decision: RuleDecision::Wanted,
            message: format!("selected preset '{preset_name}' projected into overlay state"),
            source: overlay_projection_source("selected preset branch"),
        });
    }

    if let Some(marker) = trade_marker {
        branches.push(RuleBranchExplanation {
            decision: RuleDecision::Wanted,
            message: format!(
                "trade item '{}' projected into overlay state",
                marker.trade_item_id
            ),
            source: overlay_projection_source("trade marker branch"),
        });
    }

    if branches.is_empty() {
        branches.push(RuleBranchExplanation {
            decision: RuleDecision::Unknown,
            message: "no overlay descriptor inputs available".to_string(),
            source: overlay_projection_source("empty overlay branch"),
        });
    }

    branches
}

/// Overlay layout from the transported runtime geometry: the detected
/// tooltip rect (`tooltip_window_relative`) anchors the trade panel, and
/// the per-affix row text rects (`affix_row_geometry`) anchor the affix markers,
/// joined by `row_id` — the same key the frozen rules builder
/// uses. No tooltip in the snapshot means no affix locations, so no affix
/// marker can be fabricated onto a synthetic tooltip.
fn project_rule_overlay_layout(state: &AppState) -> RuleOverlayLayout {
    let current = state.current_item();
    RuleOverlayLayout {
        // Without a detected tooltip the trade panel has no real anchor;
        // the marker itself still projects, onto an empty rect.
        tooltip_rect: current.tooltip_window_relative.unwrap_or_default(),
        // Upstream draws every affix marker at the tooltip's marker column
        // and the affix marker's row (OverlayHandler.cs:263-264). A row
        // without a marker anchor has no upstream position and draws no
        // marker; its text rect is never a substitute.
        affix_locations: current
            .affix_row_geometry
            .iter()
            .filter_map(|geometry| {
                Some(RuleAffixOverlayLocation {
                    row_id: Some(geometry.row_id),
                    geometry: geometry.marker_anchor?,
                    text: None,
                })
            })
            .collect(),
        power_location: None,
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    }
}

fn project_paragon_overlay_layout(state: &AppState) -> ParagonOverlayLayout {
    ParagonOverlayLayout {
        // The real game-window rect the overlay surface covers (see
        // `overlay_window_geometry`): the board centers on this window
        // like upstream draws on `_window` (OverlayHandler.cs:673-674).
        // The None case is unreachable from the projection (the state
        // gate runs first) and keeps the layout total for direct
        // callers.
        window_geometry: state.overlay_window_geometry.unwrap_or_default(),
        panel_left: 0,
        panel_top: 100,
        panel_height: 50,
        panel_gap: 2,
        panel_text_offset: 20,
        // Every paragon text is measured and drawn at `OverlayFontSize`
        // (OverlayHandler.cs:606), bounded to the setting's range.
        font_px: d4lc_config::bounded_overlay_font_size(state.shell.settings.overlay_font_size),
        tile_gap: 5,
        // Upstream draws the 21x21 paragon node grid in both board
        // modes (`int tileCount = 21`, OverlayHandler.cs:670/:806); a
        // 5-tile board would misplace every node of a real import.
        tile_count: 21,
        node_size_full: state.shell.settings.paragon_node_size,
        node_size_collapsed: state.shell.settings.paragon_node_size_collapsed,
        node_border_size: state.shell.settings.paragon_border_size,
        // Collapsed-mode offsets are user settings upstream reads at draw
        // time (`ParagonLeftOffsetCollapsed` -200 / `ParagonTopOffsetCollapsed`
        // -50 by default, SettingsD4.cs:72/:75, applied at
        // OverlayHandler.cs:809-812). The projection runs on every
        // descriptor rebuild, so a Paragon Config dialog edit is picked up
        // live instead of on the next start.
        collapsed_left_offset: state.shell.settings.paragon_left_offset_collapsed,
        collapsed_top_offset: state.shell.settings.paragon_top_offset_collapsed,
    }
}

pub(super) fn project_overlay_marker_mode(
    mode: d4lc_config::OverlayMarkerModeSetting,
) -> RuleOverlayMarkerMode {
    match mode {
        d4lc_config::OverlayMarkerModeSetting::ShowAll => RuleOverlayMarkerMode::ShowAll,
        d4lc_config::OverlayMarkerModeSetting::HideUnwanted => RuleOverlayMarkerMode::HideUnwanted,
    }
}

fn item_power_limit_check_ok(state: &AppState) -> bool {
    if !state.shell.settings.item_power_limit_enabled {
        return true;
    }

    // Upstream sigil exemption (OverlayHandler.cs:149-151): the item
    // power gate never applies to a sigil tooltip.
    if state
        .current_item()
        .item_type
        .as_deref()
        .is_some_and(|item_type| item_type.eq_ignore_ascii_case("sigil"))
    {
        return true;
    }

    match state.current_item().item_power {
        Some(item_power) => item_power >= state.shell.settings.item_power_limit,
        // Upstream compares `ItemPowerLimit <= _currentTooltip.ItemPower`
        // where an unparsed tooltip's item power is 0 — the gate fails
        // against any positive limit instead of silently passing.
        None => state.shell.settings.item_power_limit == 0,
    }
}

pub(super) fn overlay_projection_source(detail: &'static str) -> RuleSourceNote {
    RuleSourceNote::new(
        "crates/d4lc-app/src/app_state.rs",
        "AppState::rebuild_overlay_descriptor_state",
        detail,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
