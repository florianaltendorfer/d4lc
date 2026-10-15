//! d4lc-gui crate.

pub mod affix;
pub mod debug;
pub mod dialog;
pub mod logging;
pub mod settings;
pub mod trade;

pub use affix::editor::{merge_affix_presets, MergeColorOptions};

use d4lc_config::{
    AppSettings, DefaultColors, OverlayMarkerModeSetting, RuleToggleSettings,
    SigilDisplayModeSetting,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiTabKind {
    CurrentItem,
    AffixPresets,
    TradeImports,
    Debug,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiTab {
    pub kind: GuiTabKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemPanelState {
    pub parse_status: String,
    pub item_name: Option<String>,
    pub rarity: Option<String>,
    /// Canonical item type id of the pinned catalog (`helm`, `chest`, ...) —
    /// the value the rule evaluation compares, never the display derivation
    /// of the tooltip line. `None` when the parse resolved no type.
    pub item_type: Option<String>,
    /// Display-side base type derivation ("Helm", "Two-Handed Sword"): the
    /// trade screen's current-item card renders this; it is never an
    /// evaluation id.
    pub base_type: Option<String>,
    pub item_power: Option<u32>,
    pub affixes: Vec<CurrentItemAffixView>,
    /// Window-relative per-affix row geometry for overlay row anchoring.
    /// Filled by the live runtime event path; empty when the parse carried
    /// no row geometry.
    pub affix_row_geometry: Vec<CurrentItemAffixRowGeometryView>,
    /// Classified non-affix tooltip rows (aspect, unique aspect, rune,
    /// sigil) with their row geometry, transported from the runtime OCR
    /// parity path. Evaluation against the non-affix presets happens
    /// app-side; this list carries the parsed rows only.
    pub non_affix_rows: Vec<CurrentItemNonAffixRowView>,
    /// Detected tooltip rect in window-relative pixels for the frame the
    /// snapshot came from; `None` when no tooltip was detected (the overlay
    /// must then draw no affix markers).
    pub tooltip_window_relative: Option<d4lc_core::Rect>,
    pub non_affix_matches: Vec<CurrentItemMatchView>,
    pub rule_decision: Option<CurrentItemRuleDecisionSummary>,
    pub trade_marker: Option<TradeMarkerView>,
    pub parse_confidence: CurrentItemParseConfidence,
    pub unmatched_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixPresetPanelState {
    pub selected_preset: Option<String>,
    pub preset_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetEntryKind {
    Affix,
    Aspect,
    Unique,
    Rune,
    Sigil,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    pub kind: PresetEntryKind,
    pub id: String,
    pub label: Option<String>,
    pub item_type: Option<String>,
    pub color: String,
    /// Upstream `ItemAffix.IsAnyType`: the entry matches the affix on
    /// any item type. Kept as its own field (NOT derived from
    /// `item_type`) so an import that carries both survives an edit and
    /// a save unchanged.
    pub is_any_type: bool,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    /// Upstream `ItemAffix.TuningPrisms`: the prism names the overlay
    /// draws beside a matched affix, in catalog order.
    pub tuning_prisms: Vec<String>,
}

impl PresetEntry {
    pub fn non_affix(
        kind: PresetEntryKind,
        id: &str,
        item_type: Option<&str>,
        color: &str,
    ) -> Self {
        Self {
            kind,
            id: id.to_string(),
            label: None,
            item_type: item_type.map(str::to_string),
            color: color.to_string(),
            is_any_type: item_type.is_none(),
            is_greater: false,
            is_implicit: false,
            is_tempered: false,
            tuning_prisms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixPresetEditorPreset {
    pub name: String,
    pub entries: Vec<PresetEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarningView {
    pub provider: String,
    pub source_field: String,
    pub source_value: String,
    pub message: String,
    pub upstream_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetExportRequest {
    pub preset_name: String,
    pub suggested_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulePresetProjection {
    pub affix_presets: Vec<d4lc_rules::AffixPreset>,
    pub non_affix_presets: Vec<d4lc_rules::NonAffixPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixPresetEditorState {
    pub presets: Vec<AffixPresetEditorPreset>,
    pub selected_preset_name: Option<String>,
    pub import_warnings: Vec<ImportWarningView>,
    pub export_requests: Vec<PresetExportRequest>,
    pub rules_revision: u64,
    /// Imported paragon board routes keyed by preset name (upstream
    /// `AffixPreset.ParagonBoardsList`).
    ///
    /// The gui `AffixPresetEditorPreset` is a mixed-kind entry list and
    /// cannot hold paragon, so the routes live beside the presets in the
    /// editor state. Keeping them here (instead of in the app shell) is
    /// what makes the editor's own `rename_preset` carry the boards to
    /// the new name and `remove_preset` delete them — the same lifetime
    /// the boards have upstream, where they sit inside the preset.
    ///
    /// Empty route lists are never stored: a missing entry projects no
    /// boards either way.
    pub paragon_boards_by_preset: Vec<(String, Vec<d4lc_rules::ImportedParagonRoute>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeImportsPanelState {
    pub matched_trade_value: Option<String>,
    pub import_provider_count: usize,
    pub trade_items: Vec<TradeItemRow>,
    pub selected_trade_item_id: Option<String>,
    pub current_trade_marker: Option<TradeMarkerView>,
    pub import_state: ImportWorkflowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemAffixView {
    /// Tooltip-wide row identity, the join key to the row geometry and the
    /// rule verdicts (`d4lc_parse::ItemTooltipAffix::row_id`).
    pub row_id: d4lc_core::AffixRowId,
    pub affix_id: Option<String>,
    pub label: String,
    pub value_text: Option<String>,
    pub source_text: String,
    pub is_greater: bool,
    pub is_implicit: bool,
    pub is_tempered: bool,
    pub matched_by_rule: bool,
}

/// Window-relative geometry of one parsed affix row, transported from the
/// runtime OCR parity path for overlay row anchoring. View-model mirror of
/// `d4lc_parse::ItemTooltipAffixRowGeometry`; `row_id` names the owning
/// affix row (`CurrentItemAffixView::row_id`) and is the join key the rule
/// overlay uses between parsed affix rows and overlay row locations. The
/// segment/line indexes are provenance only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemAffixRowGeometryView {
    pub row_id: d4lc_core::AffixRowId,
    pub source_segment_index: usize,
    pub source_line_index: usize,
    pub global_line_index: usize,
    pub marker_ordinal: Option<usize>,
    /// Window-space rect of the OCR text line.
    pub text_rect: d4lc_core::Rect,
    /// Window-space affix area the row was read from.
    pub affix_area: d4lc_core::Rect,
    /// Window-space marker anchor (tooltip left edge, affix marker top and
    /// height), when the row came from an affix marker.
    pub marker_anchor: Option<d4lc_core::Rect>,
}

impl CurrentItemAffixRowGeometryView {
    /// The one projection of parsed row geometry into the view model.
    pub fn from_parse_geometry(geometry: &d4lc_parse::ItemTooltipAffixRowGeometry) -> Self {
        Self {
            row_id: geometry.row_id,
            source_segment_index: geometry.source_segment_index,
            source_line_index: geometry.source_line_index,
            global_line_index: geometry.global_line_index,
            marker_ordinal: geometry.marker_ordinal,
            text_rect: geometry.text_rect.rect(),
            affix_area: geometry.affix_area.rect(),
            marker_anchor: geometry.marker_anchor.map(|anchor| anchor.rect()),
        }
    }
}

/// Classified non-affix tooltip row (aspect, unique aspect, rune, sigil)
/// transported from the runtime OCR parity path. View-model mirror of
/// `d4lc_parse::ItemTooltipNonAffixRow`; the row's position in
/// `CurrentItemPanelState::non_affix_rows` is its identity, the line indexes
/// are provenance only (the segment-local one restarts in every segment),
/// and `rect` is the window-relative row rect composed by the runtime
/// (`None` when the row's anchor line carried no geometry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemNonAffixRowView {
    pub kind: CurrentItemMatchKind,
    pub id: Option<String>,
    pub label: Option<String>,
    pub source_line_index: usize,
    pub global_line_index: usize,
    pub rect: Option<d4lc_core::Rect>,
    /// Window-relative marker anchor (tooltip left edge, the row's marker)
    /// the overlay draws the row's marker at; `None` without a marker.
    pub marker_anchor: Option<d4lc_core::Rect>,
}

impl CurrentItemNonAffixRowView {
    /// Project one parsed non-affix row into the view model. Returns `None`
    /// for segment kinds that are not non-affix tooltip rows; the parser
    /// never emits those into `non_affix_rows`.
    pub fn from_parse_row(row: &d4lc_parse::ItemTooltipNonAffixRow) -> Option<Self> {
        let kind = match row.kind {
            d4lc_parse::TooltipOcrSegmentKind::Aspect => CurrentItemMatchKind::Aspect,
            d4lc_parse::TooltipOcrSegmentKind::UniqueAspect => CurrentItemMatchKind::Unique,
            d4lc_parse::TooltipOcrSegmentKind::Rune => CurrentItemMatchKind::Rune,
            d4lc_parse::TooltipOcrSegmentKind::Sigil => CurrentItemMatchKind::Sigil,
            _ => return None,
        };
        Some(Self {
            kind,
            id: row.id.clone(),
            label: row.label.clone(),
            source_line_index: row.source.source_line_index,
            global_line_index: row.source.global_line_index,
            rect: row.rect,
            marker_anchor: row.marker_anchor,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentItemMatchKind {
    Aspect,
    Unique,
    Rune,
    Sigil,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemMatchView {
    pub kind: CurrentItemMatchKind,
    pub id: Option<String>,
    pub label: Option<String>,
    pub source_text: Option<String>,
    pub matched_by_rule: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemRuleDecisionSummary {
    pub decision: String,
    pub active_preset_names: Vec<String>,
    pub matched_build_names: Vec<String>,
    pub selected_build_name: Option<String>,
    pub branch_messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeMarkerView {
    pub trade_item_id: String,
    pub value_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentItemParseConfidence {
    pub status: String,
    pub total_line_count: usize,
    pub unclassified_line_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeItemAffixView {
    pub affix_id: String,
    pub is_greater: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeItemRow {
    pub trade_item_id: String,
    pub item_type: String,
    pub value_text: String,
    pub affixes: Vec<TradeItemAffixView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportWorkflowStatus {
    Idle,
    Loading,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportVariantView {
    pub source_build_id: String,
    pub source_build_name: String,
    pub variant_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportUnmappedEntryView {
    pub source_field: String,
    pub source_value: String,
    pub message: String,
    pub upstream_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportResultItemView {
    pub item_type: String,
    pub affix_count: usize,
    pub aspect_count: usize,
    pub unique_count: usize,
    pub rune_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWorkflowResultView {
    pub provider: String,
    pub source_url_or_file: String,
    pub source_build_id: String,
    pub source_build_name: String,
    pub class_name: String,
    pub variant_name: Option<String>,
    pub affix_preset_name: String,
    pub items: Vec<ImportResultItemView>,
    pub paragon_board_count: usize,
    pub warnings: Vec<ImportWarningView>,
    pub unmapped_entries: Vec<ImportUnmappedEntryView>,
    pub source_trace: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWorkflowState {
    pub available_providers: Vec<String>,
    pub selected_provider: Option<String>,
    pub available_variants: Vec<ImportVariantView>,
    pub selected_variant: Option<String>,
    pub status: ImportWorkflowStatus,
    pub latest_result: Option<ImportWorkflowResultView>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugArtifactKind {
    RawCapture,
    RoiCrop,
    TooltipCrop,
    ThresholdMask,
    TemplateTrace,
    OcrText,
    ParsedItemJson,
    RuleEvaluationJson,
    OverlayStatus,
    Timings,
}

impl DebugArtifactKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RawCapture => "Raw capture",
            Self::RoiCrop => "ROI crop",
            Self::TooltipCrop => "Tooltip crop",
            Self::ThresholdMask => "Threshold / mask",
            Self::TemplateTrace => "Template trace",
            Self::OcrText => "OCR text",
            Self::ParsedItemJson => "Parsed item JSON",
            Self::RuleEvaluationJson => "Rule evaluation JSON",
            Self::OverlayStatus => "Overlay status",
            Self::Timings => "Timing metrics",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugArtifactLink {
    pub kind: DebugArtifactKind,
    pub path: String,
}

impl DebugArtifactLink {
    pub fn new(kind: DebugArtifactKind, path: &str) -> Self {
        Self {
            kind,
            path: path.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugArtifactState {
    pub kind: DebugArtifactKind,
    pub label: String,
    pub path: Option<String>,
    pub present: bool,
}

/// One Debug-screen FlipView stage panel.
///
/// Upstream binds a live `BitmapSource` per processed-screen stage
/// (`DebugView.xaml:200-247`; `DebugViewModel.cs:240-318`). The port
/// writes stage artifacts to a debug bundle instead of keeping live
/// bitmaps, so a panel carries the real bundle file for its stage when
/// the port produced one, and a note when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugStagePanel {
    pub label: String,
    /// Bundle-relative path of the stage artifact, `None` when this
    /// build produced no artifact for the stage.
    pub path: Option<String>,
    /// Explanation of the panel's source or its absence.
    pub note: String,
}

/// One row of the Debug performance table.
///
/// Upstream plots 11 LiveCharts series (`DebugViewModel.cs:236-238,589-607`);
/// the port renders the per-stage millisecond values its own pipeline
/// actually measures, so a row without a measurement stays `None`
/// instead of plotting a zero.
#[derive(Debug, Clone, PartialEq)]
pub struct DebugPerfRow {
    pub name: String,
    pub value_ms: Option<f64>,
}

/// One OCR/parse result panel.
///
/// Upstream's three OCR panels are `OcrResultAffixes`
/// (`DebugView.xaml:249-313`), `OcrResultAspect` (`:315-361`) and the
/// combined item-type/power card (`:363-419`). The port renders the
/// rows it really has and states its source (or absence) in `note`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugOcrPanel {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DebugTimingMetric {
    pub name: String,
    pub value_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DebugPanelFixture {
    pub artifact_links: Vec<DebugArtifactLink>,
    pub active_language: String,
    pub overlay_status: Option<String>,
    pub timing_metrics: Vec<DebugTimingMetric>,
    /// `Settings.IsDebugInfoEnabled` seed.
    pub debug_info_enabled: bool,
    pub stage_panels: Vec<DebugStagePanel>,
    pub perf_rows: Vec<DebugPerfRow>,
    pub ocr_panels: Vec<DebugOcrPanel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DebugPanelState {
    pub debug_artifact_count: usize,
    pub artifacts: Vec<DebugArtifactState>,
    pub active_language: String,
    pub overlay_status: Option<String>,
    pub timing_metrics: Vec<DebugTimingMetric>,
    /// `Settings.IsDebugInfoEnabled` (`SettingsD4.cs:29`): the Debug
    /// view's DebugInfo toggle, persisted on every flip
    /// (`DebugViewModel.cs:168-178`).
    pub debug_info_enabled: bool,
    /// The 8 processed-screen stage panels (`DebugView.xaml:200-247`).
    pub stage_panels: Vec<DebugStagePanel>,
    /// The performance table rows (`DebugView.xaml:445-462`).
    pub perf_rows: Vec<DebugPerfRow>,
    /// The 3 OCR result panels (`DebugView.xaml:249-419`).
    pub ocr_panels: Vec<DebugOcrPanel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPanelState {
    pub selected_system_preset: String,
    pub language: String,
    /// Culture id of the UI language (upstream `SelectedAppLanguage`);
    /// independent of the game-data `language` above.
    pub app_language: String,
    pub capture_backend: String,
    pub controller_mode: bool,
    pub debug_mode: bool,
    /// Upstream `SettingsD4.DevMode`: gates the settings screen's Debug
    /// Mode toggle (SettingsView.xaml:88,97); parsed and re-written so a
    /// hand-edited file survives a save.
    pub dev_mode: bool,
    pub check_for_updates: bool,
    pub launch_minimized: bool,
    pub minimize_to_tray: bool,
    pub paragon_mode_active: bool,
    pub active_controller_images: Vec<String>,
    pub scan_height_percent: f64,
    pub tooltip_width: u32,
    pub tooltip_max_height: u32,
    /// The four OCR area offsets (`d4lc-config::AppSettings`, upstream
    /// `SettingsD4.cs:9-12`); the runtime's OCR region assembly reads the
    /// settings values and the Debug screen's steppers edit them.
    pub affix_area_height_offset_top: i32,
    pub affix_area_height_offset_bottom: i32,
    pub affix_aspect_area_width_offset: i32,
    pub aspect_area_height_offset_top: i32,
    /// Upstream `SettingsD4.ScreenCaptureDelay` / `OverlayUpdateDelay`
    /// (live-loop pacing floors, read per tick by the app's live loop).
    pub screen_capture_delay: u32,
    pub overlay_update_delay: u32,
    pub threshold_min: u8,
    pub threshold_max: u8,
    pub threshold_similarity_tooltip: f64,
    pub threshold_similarity_affix_location: f64,
    pub threshold_similarity_aspect_location: f64,
    pub threshold_similarity_socket_location: f64,
    pub threshold_similarity_splitter_location: f64,
    pub minimal_ocr_match_type: u8,
    pub item_power_limit_enabled: bool,
    pub item_power_limit: u32,
    pub minimal_affix_value_filter_enabled: bool,
    pub multi_build_mode_enabled: bool,
    pub trade_overlay_enabled: bool,
    pub overlay_marker_mode: OverlayMarkerModeSetting,
    pub aspect_detection_enabled: bool,
    pub unique_detection_enabled: bool,
    pub rune_detection_enabled: bool,
    pub sigil_detection_enabled: bool,
    pub sigil_display_mode: SigilDisplayModeSetting,
    pub dungeon_tiers_enabled: bool,
    pub toggles: RuleToggleSettings,
    pub socket_detection_enabled: bool,
    pub tempered_detection_enabled: bool,
    pub overlay_enabled: bool,
    pub overlay_top_most: bool,
    pub overlay_font_size: u32,
    pub overlay_menu_locked: bool,
    pub mobalytics_show_browser: bool,
    pub type_area_offset_left: u32,
    /// Debug-screen numeric (`DebugViewModel.cs:444-454`):
    /// `TypeAreaOffsetRight`, upstream default 5.
    pub type_area_offset_right: u32,
    /// Debug-screen numeric (`DebugViewModel.cs:154-164`):
    /// `AspectAreaMaxHeight`, upstream default 300.
    pub aspect_area_max_height: u32,
    /// `SettingsD4.IsTuningPrismHintsEnabled` (default `false`).
    pub is_tuning_prism_hints_enabled: bool,
    /// `SettingsD4.IsImportParagonInfinityBuildsEnabled`
    /// (default `true`).
    pub is_import_paragon_infinity_builds_enabled: bool,
    /// `SettingsD4.SelectedBuildImportWebsite`
    /// (default `"Maxroll.gg"`).
    pub selected_build_import_website: String,
    /// The seven upstream `DefaultColor*` settings
    /// (`SettingsD4.cs:18-24`) as color strings. Edited by the
    /// ColorsConfig dialog; read by the add/import flows when a new
    /// preset entry is created without an explicit color.
    pub default_colors: DefaultColors,
    /// `SettingsD4.IsCollapsedParagonboardEnabled` (default `false`):
    /// the paragon overlay draws the collapsed board layout.
    pub collapsed_paragonboard_enabled: bool,
    /// `SettingsD4.ParagonBorderSize` (default 2, upstream dialog range
    /// 1..=10): node border size of the paragon overlay markers.
    pub paragon_border_size: u32,
    /// `SettingsD4.ParagonNodeSize` (default 40, range 10..=100).
    pub paragon_node_size: u32,
    /// `SettingsD4.ParagonNodeSizeCollapsed` (default 23, range 10..=100).
    pub paragon_node_size_collapsed: u32,
    /// `SettingsD4.ParagonLeftOffsetCollapsed` (default -200, range
    /// -1000..=1000): collapsed-board X offset.
    pub paragon_left_offset_collapsed: i32,
    /// `SettingsD4.ParagonTopOffsetCollapsed` (default -50, range
    /// -1000..=1000): collapsed-board Y offset.
    pub paragon_top_offset_collapsed: i32,
    /// `SettingsD4.ShowOverlayIcon` (default `false`): the overlay
    /// renders its draggable icon marker.
    pub show_overlay_icon: bool,
    /// `SettingsD4.OverlayIconPosX` / `OverlayIconPosY` (default 0,
    /// upstream slider range 0..=1000).
    pub overlay_icon_pos_x: u32,
    pub overlay_icon_pos_y: u32,
    /// Local crash-log diagnostics export (default `false`, opt-in).
    /// When enabled, the Debug screen may copy a local crash log to
    /// a folder the user attaches to a GitHub issue they open
    /// manually. The app never uploads anything.
    pub diagnostics_export_enabled: bool,
    /// Check-only release banner (default `true`). When enabled, the
    /// app checks GitHub Releases for a newer version and shows a
    /// non-blocking banner with the release link. The check never
    /// downloads, installs, or uploads anything.
    pub update_check_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsChangeEffects {
    pub selected_language: Option<String>,
    pub active_language: String,
    pub ocr_profile_rebuild_required: bool,
    pub fuzzy_index_rebuild_required: bool,
    pub localized_parser_reload_required: bool,
    pub localized_import_reload_required: bool,
    pub localized_paragon_reload_required: bool,
    pub reload_error: Option<LanguageReloadError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageReloadError {
    pub language: String,
    pub missing_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuiShellState {
    pub window_title: String,
    pub tabs: Vec<GuiTab>,
    pub active_tab: GuiTabKind,
    pub current_item: CurrentItemPanelState,
    pub affix_presets: AffixPresetPanelState,
    pub trade_imports: TradeImportsPanelState,
    pub debug: DebugPanelState,
    pub settings: SettingsPanelState,
    pub owns_capture_logic: bool,
    pub owns_ocr_logic: bool,
    pub owns_parser_logic: bool,
    pub owns_rule_logic: bool,
    pub owns_import_logic: bool,
    pub owns_overlay_logic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiShellFixture {
    pub window_title: String,
    pub current_item_name: Option<String>,
    pub selected_affix_preset: Option<String>,
    pub matched_trade_value: Option<String>,
    pub debug_artifact_count: usize,
    pub selected_system_preset: String,
}

impl GuiShellState {
    pub fn default_startup() -> Self {
        Self::from_fixture(GuiShellFixture {
            window_title: "d4lc".to_string(),
            current_item_name: None,
            selected_affix_preset: None,
            matched_trade_value: None,
            debug_artifact_count: 0,
            selected_system_preset: "1440p_SMF".to_string(),
        })
    }

    pub fn from_fixture(fixture: GuiShellFixture) -> Self {
        let mut current_item = CurrentItemPanelState::empty();
        current_item.item_name = fixture.current_item_name;
        let mut trade_imports = TradeImportsPanelState::with_providers(default_import_providers());
        trade_imports.matched_trade_value = fixture.matched_trade_value;
        Self {
            window_title: fixture.window_title,
            tabs: required_tabs(),
            active_tab: GuiTabKind::CurrentItem,
            current_item,
            affix_presets: AffixPresetPanelState {
                selected_preset: fixture.selected_affix_preset,
                preset_count: 0,
            },
            trade_imports,
            debug: {
                let mut debug = DebugPanelState::from_fixture(DebugPanelFixture {
                    artifact_links: Vec::new(),
                    active_language: AppSettings::default().selected_affix_language,
                    overlay_status: None,
                    timing_metrics: Vec::new(),
                    debug_info_enabled: false,
                    stage_panels: Vec::new(),
                    perf_rows: Vec::new(),
                    ocr_panels: Vec::new(),
                });
                debug.debug_artifact_count = fixture.debug_artifact_count;
                debug
            },
            settings: {
                let mut settings =
                    SettingsPanelState::from_config(&AppSettings::default(), Vec::new());
                settings.selected_system_preset = fixture.selected_system_preset;
                settings
            },
            owns_capture_logic: false,
            owns_ocr_logic: false,
            owns_parser_logic: false,
            owns_rule_logic: false,
            owns_import_logic: false,
            owns_overlay_logic: false,
        }
    }

    pub fn apply_language_change(&mut self, language: &str) -> SettingsChangeEffects {
        let effects = self.settings.set_language(language);
        self.debug.active_language = effects.active_language.clone();
        effects
    }
}

impl CurrentItemPanelState {
    pub fn empty() -> Self {
        Self {
            parse_status: "not_item".to_string(),
            item_name: None,
            rarity: None,
            item_type: None,
            base_type: None,
            item_power: None,
            affixes: Vec::new(),
            affix_row_geometry: Vec::new(),
            non_affix_rows: Vec::new(),
            tooltip_window_relative: None,
            non_affix_matches: Vec::new(),
            rule_decision: None,
            trade_marker: None,
            parse_confidence: CurrentItemParseConfidence {
                status: "not_item".to_string(),
                total_line_count: 0,
                unclassified_line_count: 0,
            },
            unmatched_lines: Vec::new(),
        }
    }

    pub fn from_parse_and_rules(
        parse: &d4lc_parse::ItemTooltipParseResult,
        rule: Option<&d4lc_rules::RuleEvaluationResult>,
    ) -> Self {
        let matched_affix_ids = rule
            .map(|result| {
                result
                    .matched_affixes
                    .iter()
                    .map(|affix| affix.affix_id.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut non_affix_matches = Vec::new();

        if let Some(power) = parse.legendary_power.as_ref() {
            let matched_by_rule = rule
                .and_then(|result| result.matched_power.as_ref())
                .is_some_and(|matched| {
                    power.power_id.as_deref() == Some(matched.power_id.as_str())
                });
            non_affix_matches.push(CurrentItemMatchView {
                kind: if power.is_unique {
                    CurrentItemMatchKind::Unique
                } else {
                    CurrentItemMatchKind::Aspect
                },
                id: power.power_id.clone(),
                label: power.power_label.clone(),
                source_text: Some(power.raw_text.clone()),
                matched_by_rule,
            });
        } else if let Some(matched) = rule.and_then(|result| result.matched_power.as_ref()) {
            non_affix_matches.push(CurrentItemMatchView {
                kind: match matched.power_kind {
                    d4lc_rules::RulePowerKind::Aspect => CurrentItemMatchKind::Aspect,
                    d4lc_rules::RulePowerKind::Unique => CurrentItemMatchKind::Unique,
                },
                id: Some(matched.power_id.clone()),
                label: None,
                source_text: None,
                matched_by_rule: true,
            });
        }

        for rune_id in rule
            .map(|result| result.matched_runes.as_slice())
            .unwrap_or(&[])
        {
            non_affix_matches.push(CurrentItemMatchView {
                kind: CurrentItemMatchKind::Rune,
                id: Some(rune_id.clone()),
                label: None,
                source_text: None,
                matched_by_rule: true,
            });
        }

        if let Some(sigil_id) = rule.and_then(|result| result.matched_sigil.as_ref()) {
            non_affix_matches.push(CurrentItemMatchView {
                kind: CurrentItemMatchKind::Sigil,
                id: Some(sigil_id.clone()),
                label: None,
                source_text: None,
                matched_by_rule: true,
            });
        }

        Self {
            parse_status: parse.status.as_str().to_string(),
            item_name: parse.display_name.clone(),
            rarity: parse.rarity.map(|rarity| rarity.as_str().to_string()),
            // The evaluation identity is the canonical catalog type id; the
            // descriptive derivation of the tooltip line stays a display
            // field of its own.
            item_type: parse.item_type_id.clone(),
            base_type: parse.base_type.clone(),
            item_power: parse.item_power,
            affixes: parse
                .affixes
                .iter()
                .map(|affix| CurrentItemAffixView {
                    row_id: affix.row_id,
                    affix_id: affix.affix_id.clone(),
                    label: affix
                        .affix_label
                        .clone()
                        .unwrap_or_else(|| affix.name.clone()),
                    value_text: affix.value_text.clone(),
                    source_text: affix.raw_text.clone(),
                    is_greater: affix.is_greater_affix,
                    is_implicit: affix.is_implicit,
                    is_tempered: affix.is_tempered,
                    matched_by_rule: affix
                        .affix_id
                        .as_deref()
                        .is_some_and(|affix_id| matched_affix_ids.contains(&affix_id)),
                })
                .collect(),
            // The runtime OCR parity path fills the row geometry inside the
            // parse result; the pure parser leaves it empty (absence).
            // The window-relative tooltip rect only exists on the live
            // runtime event path, so it stays `None` here.
            affix_row_geometry: parse
                .affix_row_geometry
                .iter()
                .map(CurrentItemAffixRowGeometryView::from_parse_geometry)
                .collect(),
            non_affix_rows: parse
                .non_affix_rows
                .iter()
                .filter_map(CurrentItemNonAffixRowView::from_parse_row)
                .collect(),
            tooltip_window_relative: None,
            non_affix_matches,
            rule_decision: rule.map(|result| CurrentItemRuleDecisionSummary {
                decision: result.final_decision.as_str().to_string(),
                active_preset_names: result.explanation.active_preset_names.clone(),
                matched_build_names: result.explanation.matched_build_names.clone(),
                selected_build_name: result.explanation.selected_build_name.clone(),
                branch_messages: result
                    .explanation
                    .branches
                    .iter()
                    .map(|branch| branch.message.clone())
                    .collect(),
            }),
            trade_marker: rule
                .and_then(|result| result.trade_marker.as_ref())
                .map(trade_marker_view),
            parse_confidence: CurrentItemParseConfidence {
                status: parse.status.as_str().to_string(),
                total_line_count: parse.source.line_count,
                unclassified_line_count: parse.unclassified_item_lines.len(),
            },
            unmatched_lines: parse
                .unclassified_item_lines
                .iter()
                .map(|line| line.raw_text.clone())
                .collect(),
        }
    }
}

/// Split a trailing `_N` decimal suffix off a trade item id:
/// `Trade_A_10` → `("Trade_A", Some("10"))`, `Trade_A` →
/// `("Trade_A", None)`. A non-numeric tail is not a suffix.
fn split_trade_id_suffix(id: &str) -> (&str, Option<&str>) {
    match id.rsplit_once('_') {
        Some((base, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (base, Some(suffix))
        }
        _ => (id, None),
    }
}

/// Insert order for wanted trade rows must match the id synthesis
/// order (`Trade_<Type>`, then `_2`, `_3`, ... in file order — the
/// `d4lc_config::synthesize_trade_item_id` rule): a save persists this
/// panel's order and a load re-synthesizes ids in file order, so any
/// other order rotates the id↔row mapping once ten same-type rows
/// exist (`Trade_A_10` byte-sorts before `Trade_A_2`). The `_N`
/// suffix therefore compares numerically.
fn trade_item_id_insert_order(left: &str, right: &str) -> std::cmp::Ordering {
    let (left_base, left_suffix) = split_trade_id_suffix(left);
    let (right_base, right_suffix) = split_trade_id_suffix(right);
    left_base.cmp(right_base).then_with(|| {
        match (left_suffix, right_suffix) {
            // Compare digit strings by length, then lexically: exact
            // numeric order without parse overflow.
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(left_digits), Some(right_digits)) => left_digits
                .len()
                .cmp(&right_digits.len())
                .then_with(|| left_digits.cmp(right_digits)),
        }
    })
}

impl TradeImportsPanelState {
    pub fn with_providers(providers: Vec<String>) -> Self {
        let import_provider_count = providers.len();
        Self {
            matched_trade_value: None,
            import_provider_count,
            trade_items: Vec::new(),
            selected_trade_item_id: None,
            current_trade_marker: None,
            import_state: ImportWorkflowState {
                available_providers: providers,
                selected_provider: None,
                available_variants: Vec::new(),
                selected_variant: None,
                status: ImportWorkflowStatus::Idle,
                latest_result: None,
                last_error: None,
            },
        }
    }

    pub fn upsert_trade_item(&mut self, trade_item: TradeItemRow) {
        self.selected_trade_item_id = Some(trade_item.trade_item_id.clone());
        if let Some(existing) = self
            .trade_items
            .iter_mut()
            .find(|existing| existing.trade_item_id == trade_item.trade_item_id)
        {
            *existing = trade_item;
        } else {
            self.trade_items.push(trade_item);
            self.trade_items.sort_by(|left, right| {
                trade_item_id_insert_order(&left.trade_item_id, &right.trade_item_id)
            });
        }
    }

    pub fn remove_trade_item(&mut self, trade_item_id: &str) -> Result<(), String> {
        let initial_len = self.trade_items.len();
        self.trade_items
            .retain(|trade_item| trade_item.trade_item_id != trade_item_id);
        if self.trade_items.len() == initial_len {
            return Err(format!("trade item '{trade_item_id}' not found"));
        }
        if self.selected_trade_item_id.as_deref() == Some(trade_item_id) {
            self.selected_trade_item_id = self
                .trade_items
                .first()
                .map(|trade_item| trade_item.trade_item_id.clone());
        }
        Ok(())
    }

    pub fn apply_rule_evaluation(&mut self, result: &d4lc_rules::RuleEvaluationResult) {
        self.matched_trade_value = result
            .trade_marker
            .as_ref()
            .map(|marker| marker.value_text.clone());
        self.current_trade_marker = result.trade_marker.as_ref().map(trade_marker_view);
    }

    pub fn apply_import_state(&mut self, import_state: ImportWorkflowState) {
        self.import_provider_count = import_state.available_providers.len();
        self.import_state = import_state;
    }
}

impl DebugPanelState {
    pub fn from_fixture(fixture: DebugPanelFixture) -> Self {
        let artifacts = required_debug_artifacts()
            .iter()
            .copied()
            .map(|kind| debug_artifact_state(kind, &fixture.artifact_links))
            .collect::<Vec<_>>();
        let debug_artifact_count = artifacts.iter().filter(|artifact| artifact.present).count();
        Self {
            debug_artifact_count,
            artifacts,
            active_language: fixture.active_language,
            overlay_status: fixture.overlay_status,
            timing_metrics: fixture.timing_metrics,
            debug_info_enabled: fixture.debug_info_enabled,
            stage_panels: fixture.stage_panels,
            perf_rows: fixture.perf_rows,
            ocr_panels: fixture.ocr_panels,
        }
    }

    pub fn artifact(&self, kind: DebugArtifactKind) -> Option<&DebugArtifactState> {
        self.artifacts.iter().find(|artifact| artifact.kind == kind)
    }
}

pub const fn required_debug_artifacts() -> &'static [DebugArtifactKind] {
    &[
        DebugArtifactKind::RawCapture,
        DebugArtifactKind::RoiCrop,
        DebugArtifactKind::TooltipCrop,
        DebugArtifactKind::ThresholdMask,
        DebugArtifactKind::TemplateTrace,
        DebugArtifactKind::OcrText,
        DebugArtifactKind::ParsedItemJson,
        DebugArtifactKind::RuleEvaluationJson,
        DebugArtifactKind::OverlayStatus,
        DebugArtifactKind::Timings,
    ]
}

pub fn is_debug_output_path(path: &str) -> bool {
    path == "debug-output" || path.starts_with("debug-output/")
}

fn debug_artifact_state(
    kind: DebugArtifactKind,
    links: &[DebugArtifactLink],
) -> DebugArtifactState {
    let path = links
        .iter()
        .find(|link| link.kind == kind && is_debug_output_path(&link.path))
        .map(|link| link.path.clone());
    DebugArtifactState {
        kind,
        label: kind.label().to_string(),
        present: path.is_some(),
        path,
    }
}

fn default_import_providers() -> Vec<String> {
    vec![
        "D2Core".to_string(),
        "D4Builds".to_string(),
        "Maxroll".to_string(),
        "Mobalytics".to_string(),
    ]
}

fn trade_marker_view(marker: &d4lc_rules::RuleTradeMarker) -> TradeMarkerView {
    TradeMarkerView {
        trade_item_id: marker.trade_item_id.clone(),
        value_text: marker.value_text.clone(),
    }
}

pub fn required_tabs() -> Vec<GuiTab> {
    vec![
        GuiTab {
            kind: GuiTabKind::CurrentItem,
            label: "Current item".to_string(),
        },
        GuiTab {
            kind: GuiTabKind::AffixPresets,
            label: "Affix presets".to_string(),
        },
        GuiTab {
            kind: GuiTabKind::TradeImports,
            label: "Trade / imports".to_string(),
        },
        GuiTab {
            kind: GuiTabKind::Debug,
            label: "Debug".to_string(),
        },
        GuiTab {
            kind: GuiTabKind::Settings,
            label: "Settings".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests;
