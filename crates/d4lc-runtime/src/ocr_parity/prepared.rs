//! The prepared OCR session and the entry points that run it.

use std::path::Path;
use std::time::Instant;

use d4lc_assets::AssetRepository;
use d4lc_config::load_app_settings;
use d4lc_core::coords::{FramePlacement, TooltipPlacement};
use d4lc_core::{CaptureFrame, OcrText};
use d4lc_fuzzy::FuzzyIndexes;
use d4lc_ocr::{
    plan_ocr_segments, upstream_conversion_route, upstream_converted_text_for_route,
    OcrEngineRegistry, OcrMarker, OcrOptions, OcrProfile, OcrSegmentKind, OcrSegmentPlan,
    OcrSegmentPlanInput, TooltipOcrRegionConfig,
};
use d4lc_parse::convert_to_item_type;

use super::detection::{
    detect_upstream_markers_notifying, input_rect, is_top_splitter_marker,
    prepare_tooltip_ocr_input,
};
use super::parallel::{elapsed_ms, join_scoped, parallel_map};
use super::result::runtime_parity_result;
use super::templates::{
    add_supplement_marker_templates, load_marker_templates, load_selected_tooltip_templates,
    tooltip_region_config, MarkerTemplate, TooltipTemplate,
};
use super::types::{
    ocr_input_trace, segment_ocr_image, DetectedMarkerPlan, OcrInputTrace, SegmentResult,
    TooltipCropTrace, TooltipOcrExecution, TooltipOcrParityResult, TooltipOcrPreprocessVariant,
};
use super::UPSTREAM_TOOLTIP_CROP_RULE;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the complete upstream-parity OCR pipeline on a single frame crop.
///
/// This is the shared entry point used by the CLI (`read_tooltip`) and the
/// app live-loop (`OcrWorker`) when the app config root and the game-data
/// root coincide (both run against the checkout or the installed bundle
/// root). The desktop app's live runtime resolves its two roots separately
/// through [`PreparedTooltipOcr::prepare_with_game_data_root`]. The frame is
/// treated as a whole 1:1 window capture, so row geometry is frame-relative.
pub fn process_tooltip_ocr_parity(
    root: &Path,
    frame: CaptureFrame,
    preset: String,
    profile: OcrProfile,
    registry: &OcrEngineRegistry,
    preview_chars: usize,
) -> Result<TooltipOcrParityResult, String> {
    let prepared = PreparedTooltipOcr::prepare(root, preset, profile)?;
    process_tooltip_ocr_parity_prepared(
        &prepared,
        registry,
        &frame,
        preview_chars,
        &TooltipPlacement::whole_frame(FramePlacement::default(), frame.width, frame.height),
    )
}

/// Run the tooltip OCR parity path with session state prepared once.
///
/// `placement` says where `frame` sits in the captured frame and window; the
/// row geometry of the result is converted into window space through it,
/// once.
///
/// Callers that OCR many frames prepare once with
/// [`PreparedTooltipOcr::prepare`]; results are identical to the one-shot
/// [`process_tooltip_ocr_parity`] for the same inputs because the prepared
/// state is a pure cache of settings, template images, and fuzzy indexes.
pub fn process_tooltip_ocr_parity_prepared(
    prepared: &PreparedTooltipOcr,
    registry: &OcrEngineRegistry,
    frame: &CaptureFrame,
    preview_chars: usize,
    placement: &TooltipPlacement,
) -> Result<TooltipOcrParityResult, String> {
    let execution = prepared.execute(registry, frame)?;
    Ok(runtime_parity_result(
        &execution,
        preview_chars,
        prepared.fuzzy_indexes.as_ref(),
        placement,
    ))
}

// ---------------------------------------------------------------------------
// Prepared OCR session state
// ---------------------------------------------------------------------------

/// Segments recognized at once. Each one holds a pooled Tesseract engine
/// (about 30 MB), and the upper tooltip section, recognized first, is the
/// longest segment: more threads add memory, not speed. Callers that keep
/// a pool for this session warm it with this many engines.
pub const OCR_SEGMENT_THREADS: usize = 4;

/// Session-stable OCR state: app settings, the marker and
/// tooltip template images, and the fuzzy candidate indexes, loaded once per
/// OCR worker session and reused read-only for every tooltip job.
///
/// This mirrors the upstream `OcrHandler`/`ScreenProcessHandler` shape, which
/// holds templates and settings as long-lived handler state instead of
/// reloading them per conversion. The region geometry comes from the settings
/// alone (upstream keeps TooltipWidth/TooltipMaxHeight and the four area
/// offsets on `SettingsD4`, seeded from the preset's config on a preset
/// change) — the preset config is not read here.
pub struct PreparedTooltipOcr {
    preset: String,
    profile: OcrProfile,
    settings: d4lc_config::AppSettings,
    marker_templates: Vec<MarkerTemplate>,
    tooltip_templates: Vec<TooltipTemplate>,
    /// Which pixels the OCR steps of this session recognize
    /// ([`TooltipOcrPreprocessVariant`]).
    preprocess_variant: TooltipOcrPreprocessVariant,
    /// Upstream-parity fuzzy candidate indexes. Upstream loads this data once
    /// per handler session (`OcrHandler.InitAffixData`/`InitRuneData`/
    /// `InitSigilData`), not per conversion. `None` (failed load) leaves the
    /// session unusable: [`Self::catalog_error`] names the cause, and both
    /// the readiness check and every execution refuse to run on it.
    fuzzy_indexes: Option<FuzzyIndexes>,
    /// Why the fuzzy indexes could not be loaded.
    fuzzy_index_error: Option<String>,
}

impl PreparedTooltipOcr {
    /// Why the catalog data behind the fuzzy indexes could not be loaded
    /// for the settings language; `None` when it is loaded.
    pub fn catalog_error(&self) -> Option<&str> {
        self.fuzzy_index_error.as_deref()
    }

    /// The OCR input preprocessing variant of this session.
    pub fn preprocess_variant(&self) -> TooltipOcrPreprocessVariant {
        self.preprocess_variant
    }

    /// The session's loaded fuzzy candidate indexes (`None` when the catalog
    /// data could not be loaded; see [`Self::catalog_error`]).
    ///
    /// Exposed for callers that compose [`runtime_parity_result`] themselves
    /// from a session execution.
    pub fn fuzzy_indexes(&self) -> Option<&FuzzyIndexes> {
        self.fuzzy_indexes.as_ref()
    }

    /// The same session with a different OCR input preprocessing variant.
    ///
    /// Used by the same-capture comparison that decided the default
    /// ([`TooltipOcrPreprocessVariant`]); the live path keeps the default.
    pub fn with_preprocess_variant(mut self, variant: TooltipOcrPreprocessVariant) -> Self {
        self.preprocess_variant = variant;
        self
    }

    /// Load settings and template images once per OCR session.
    ///
    /// Single-root entry point: the app config root and the game-data root
    /// coincide because the caller runs against the checkout or the installed
    /// bundle root (CLI `read_tooltip`, CLI live loop, tests).
    pub fn prepare(root: &Path, preset: String, profile: OcrProfile) -> Result<Self, String> {
        Self::prepare_with_game_data_root(root, root, preset, profile)
    }

    /// Load settings and template images once per OCR session from the app's
    /// two resolved roots.
    ///
    /// `config_root` carries the app settings (`Config/Settings.json`), the
    /// controller config, and the extracted user presets
    /// (`Images/{preset}` — `d4lc_assets::EXTRACTED_PRESETS_DIR`).
    /// `game_data_root` carries the vendored `assets/original` tree: the
    /// localized data behind the fuzzy candidate indexes, and the vendored
    /// `assets/original/systempresets` fallback for a preset the config root
    /// cannot serve. Passing the same path for both roots reproduces
    /// [`Self::prepare`] byte for byte.
    pub fn prepare_with_game_data_root(
        config_root: &Path,
        game_data_root: &Path,
        preset: String,
        profile: OcrProfile,
    ) -> Result<Self, String> {
        let settings = load_app_settings(config_root)?;
        let config_repo = AssetRepository::new(config_root);
        let game_data_repo = AssetRepository::new(game_data_root);
        // The preset resolutions the config root can serve stay exactly as
        // they were: the extracted `Images/{preset}` copy wins, then the
        // vendored tree inside the config root. Only a preset the config root
        // cannot serve at all falls back to the vendored tree under the
        // resolved game-data root — the same two-root precedence the app's
        // preset lists use (`d4lc-app` `actions::settings::preset_asset_roots`).
        let preset_in_config_root = [
            d4lc_assets::EXTRACTED_PRESETS_DIR,
            d4lc_assets::SYSTEM_PRESETS_DIR,
        ]
        .iter()
        .any(|dir| config_root.join(dir).join(&preset).is_dir());
        let preset_repo = if preset_in_config_root {
            &config_repo
        } else {
            &game_data_repo
        };
        let loaded = preset_repo.load_preset_templates(&preset)?;
        let mut marker_templates = load_marker_templates(
            preset_repo,
            &loaded.templates,
            settings.threshold_min,
            settings.threshold_max,
        )?;
        add_supplement_marker_templates(
            &mut marker_templates,
            &game_data_repo,
            &preset,
            settings.threshold_min,
            settings.threshold_max,
        )?;
        let tooltip_templates = load_selected_tooltip_templates(
            preset_repo,
            config_root,
            &loaded.templates,
            &preset,
            &profile.language,
            &settings,
        )?;
        // Upstream loads fuzzy candidate data once per handler session from the
        // settings affix language (OcrHandler.InitAffixData/InitRuneData/
        // InitSigilData), not per conversion. A missing or broken language
        // pack is kept as `fuzzy_index_error`: preparing still succeeds so
        // the caller can report the cause, but readiness and every
        // execution refuse to run without the catalog.
        let mut fuzzy_index_error = None;
        let fuzzy_indexes = match FuzzyIndexes::load_with_settings(
            &game_data_repo,
            &settings.selected_affix_language,
            &settings,
        ) {
            Ok(indexes) => Some(indexes),
            Err(error) => {
                eprintln!(
                    "d4lc-runtime: fuzzy index load failed for language {}: {error}; the OCR session is not usable without the catalog data",
                    settings.selected_affix_language
                );
                // Reported as a discard with its concrete reason when the OCR
                // worker runs this inside a job (`with_job_context_scope`); a
                // caller outside a job has no session to report into.
                if let Some(job) = crate::diagnostics::current_job_context() {
                    let diagnostics = job.context();
                    let event = diagnostics
                        .begin(
                            job.frame_id(),
                            crate::diagnostics::DiagnosticsPhase::OcrFinish,
                        )
                        .with_outcome(crate::diagnostics::DiagnosticsOutcome::discarded(
                            crate::diagnostics::DiscardReason::CatalogUnavailable,
                        ));
                    let language = settings.selected_affix_language.clone();
                    let event = diagnostics
                        .attach_detail(event, || format!("language={language} error={error}"));
                    diagnostics.record(event);
                }
                fuzzy_index_error = Some(format!(
                    "language {} from {}: {error}",
                    settings.selected_affix_language,
                    game_data_root.display()
                ));
                None
            }
        };

        let preprocess_variant = TooltipOcrPreprocessVariant::for_settings(&settings);
        Ok(Self {
            preset,
            profile,
            settings,
            marker_templates,
            tooltip_templates,
            preprocess_variant,
            fuzzy_indexes,
            fuzzy_index_error,
        })
    }

    /// Execute the per-frame OCR work: crop preparation, marker detection,
    /// segment planning, Tesseract recognition and upstream conversion.
    ///
    /// Fails when the session carries no catalog data: without the candidate
    /// indexes no affix, aspect or item type resolves, and an item that only
    /// looks evaluated must never be produced. Every evaluation
    /// path runs through here, so the check cannot be bypassed by a caller
    /// that uses the prepared session instead of the one-shot entry point.
    pub fn execute(
        &self,
        registry: &OcrEngineRegistry,
        frame: &CaptureFrame,
    ) -> Result<TooltipOcrExecution, String> {
        if let Some(error) = self.catalog_error() {
            return Err(format!("catalog data unavailable: {error}"));
        }
        let (ocr_frame, crop_trace) = prepare_tooltip_ocr_input(
            frame,
            &self.tooltip_templates,
            // Upstream crops by the stored `Settings.TooltipWidth`, seeded
            // from the preset on a preset change (ScreenProcessHandler.cs:670,
            // SettingsViewModel.cs:272).
            i32::try_from(self.settings.tooltip_width)
                .map_err(|_| "TooltipWidth must be inside 0..=i32::MAX".to_string())?,
            self.settings.threshold_similarity_tooltip,
        )?;
        self.execute_tooltip_crop(registry, ocr_frame, crop_trace)
    }

    /// [`Self::execute`] for a `crop` the live detector already cut with
    /// upstream's FindTooltips rule. Upstream OCRs that `_currentScreenTooltip`
    /// directly, so no second tooltip search runs; the crop keeps the frame
    /// geometry a search without a match would give it.
    pub fn execute_detected_crop(
        &self,
        registry: &OcrEngineRegistry,
        crop: &CaptureFrame,
    ) -> Result<TooltipOcrExecution, String> {
        if let Some(error) = self.catalog_error() {
            return Err(format!("catalog data unavailable: {error}"));
        }
        let whole = input_rect(crop);
        let crop_trace = TooltipCropTrace {
            planned_tooltip_roi: whole,
            search_roi: whole,
            detected_tooltip_rect: None,
            final_tooltip_crop_rect: whole,
            crop_source: "detector_final_crop",
            upstream_rule: UPSTREAM_TOOLTIP_CROP_RULE,
            original_input_size: (crop.width, crop.height),
            final_tooltip_size: (crop.width, crop.height),
        };
        self.execute_tooltip_crop(registry, crop.clone(), crop_trace)
    }

    /// Marker detection, segment planning, recognition and conversion of
    /// the final tooltip crop.
    fn execute_tooltip_crop(
        &self,
        registry: &OcrEngineRegistry,
        ocr_frame: CaptureFrame,
        crop_trace: TooltipCropTrace,
    ) -> Result<TooltipOcrExecution, String> {
        let region_config = tooltip_region_config(
            &self.marker_templates,
            ocr_frame.width,
            ocr_frame.height,
            &self.settings,
        )?;
        // The upper tooltip section is the longest segment, and its region
        // depends on the splitters alone (`upper_tooltip_region`): it is
        // recognized from the moment the splitter search is done, while the
        // other marker searches and segments still run.
        let frame = &ocr_frame;
        let plan_with_splitters_only = |splitter_markers: Vec<OcrMarker>| OcrSegmentPlanInput {
            config: region_config,
            affix_markers: Vec::new(),
            aspect_marker: None,
            socket_markers: Vec::new(),
            has_tooltip_top_splitter: splitter_markers.iter().any(is_top_splitter_marker),
            splitter_markers,
            language: self.profile.language.clone(),
            socket_detection_enabled: self.settings.is_socket_detection_enabled,
            tempered_detection_enabled: self.settings.is_tempered_affix_detection_enabled,
            aspect_detection_enabled: self.settings.rule_settings.aspect_detection_enabled,
            unique_detection_enabled: self.settings.rule_settings.unique_detection_enabled,
            rune_detection_enabled: self.settings.rule_settings.rune_detection_enabled,
            item_type: None,
            is_unique_item: false,
        };
        let (splitters_tx, splitters_rx) = std::sync::mpsc::channel::<Vec<OcrMarker>>();
        let (marker_plan, early_upper, tail_first_pass) = std::thread::scope(|scope| {
            let early_upper = scope.spawn(move || {
                let splitters = splitters_rx.recv().ok()?;
                let plan = plan_ocr_segments(plan_with_splitters_only(splitters))
                    .into_iter()
                    .find(|plan| plan.kind == OcrSegmentKind::UpperTooltip)?;
                let result = self.execute_segment(frame, registry, plan.clone());
                Some((plan, result))
            });
            let marker_plan = detect_upstream_markers_notifying(
                frame,
                &self.marker_templates,
                self.settings.threshold_min,
                self.settings.threshold_max,
                &self.settings,
                |splitters| {
                    // A receiver that is gone has nothing left to start.
                    let _ = splitters_tx.send(splitters.to_vec());
                },
            );
            let tail_first_pass = marker_plan.as_ref().ok().map(|marker_plan| {
                let tail = first_pass_tail_plans(self.segment_plan_input(
                    region_config,
                    marker_plan,
                    None,
                ));
                self.execute_segments_parallel(
                    frame,
                    registry,
                    tail,
                    OCR_SEGMENT_THREADS.saturating_sub(1).max(1),
                )
            });
            (marker_plan, join_scoped(early_upper), tail_first_pass)
        });
        let marker_plan = marker_plan?;
        let tail_first_pass = tail_first_pass.unwrap_or_default();
        let segment_input = |item_type: Option<String>| {
            self.segment_plan_input(region_config, &marker_plan, item_type)
        };

        // Upstream ProcessTooltip order: FindItemTypesPower OCRs the upper
        // tooltip section first and resolves the item type from its text
        // (OcrHandler.ConvertToItemType); FindItemAffixes then converts each
        // affix area with the route that item type selects
        // (ScreenProcessHandler.FindItemAffix: sigil -> ConvertToSigil,
        // rune -> ConvertToRune, otherwise affix). The marker-derived
        // segments are therefore planned twice: once to locate the upper
        // tooltip section, and once with the resolved item type.
        //
        // Upstream recognizes the affix areas in parallel (Parallel.For in
        // FindItemAffixes, beside the aspect in Parallel.Invoke). Here every
        // segment of the first plan runs at once, the upper section included:
        // a segment of the second plan reuses the recognition of a
        // first-plan segment with the same pixels and the same recognition
        // options (`same_recognition`) and is only converted again under
        // its own plan, so a segment is recognized after the upper section
        // only when the item type moved or added it.
        let upper_plans = plan_ocr_segments(segment_input(None))
            .into_iter()
            .filter(|plan| plan.kind == OcrSegmentKind::UpperTooltip);
        let mut early_upper = early_upper;
        let mut results = Vec::with_capacity(tail_first_pass.len() + 1);
        let mut upper_tooltip_raw_text: Option<String> = None;
        for plan in upper_plans {
            // The early recognition counts only for the plan the complete
            // marker set yields.
            let result = match early_upper.take() {
                Some((early_plan, result)) if early_plan == plan => result?,
                _ => self.execute_segment(&ocr_frame, registry, plan)?,
            };
            upper_tooltip_raw_text = Some(result.text.raw_text.clone());
            results.push(result);
        }
        let mut tail_first_pass = tail_first_pass;

        let tail_plans = match upper_tooltip_raw_text.as_deref() {
            Some(raw_text) => replan_tail_segments_with_resolved_item_type(
                segment_input,
                raw_text,
                self.fuzzy_indexes.as_ref(),
            ),
            None => plan_ocr_segments(segment_input(None))
                .into_iter()
                .filter(|plan| plan.kind != OcrSegmentKind::UpperTooltip)
                .collect(),
        };
        let mut missing = Vec::new();
        for plan in tail_plans {
            match tail_first_pass
                .iter()
                .position(|(recognized, _)| same_recognition(recognized, &plan))
            {
                Some(position) => {
                    let first = tail_first_pass.swap_remove(position).1?;
                    results.push(self.convert_segment(
                        plan,
                        first.text,
                        first.tsv,
                        first.ocr_input,
                        first.ocr_ms,
                    ));
                }
                None => missing.push(plan),
            }
        }
        for (_, result) in
            self.execute_segments_parallel(&ocr_frame, registry, missing, OCR_SEGMENT_THREADS)
        {
            results.push(result?);
        }
        results.sort_by_key(|left| left.plan.index);

        Ok(TooltipOcrExecution {
            profile: self.profile.clone(),
            preset: self.preset.clone(),
            ocr_frame,
            preprocess_variant: self.preprocess_variant,
            crop_trace,
            marker_plan,
            results,
        })
    }

    /// The segment planner's input for the detected markers.
    fn segment_plan_input(
        &self,
        config: TooltipOcrRegionConfig,
        marker_plan: &DetectedMarkerPlan,
        item_type: Option<String>,
    ) -> OcrSegmentPlanInput {
        OcrSegmentPlanInput {
            config,
            affix_markers: marker_plan.affix_markers.clone(),
            aspect_marker: marker_plan.aspect_marker.clone(),
            socket_markers: marker_plan.socket_markers.clone(),
            splitter_markers: marker_plan.splitter_markers.clone(),
            language: self.profile.language.clone(),
            socket_detection_enabled: self.settings.is_socket_detection_enabled,
            tempered_detection_enabled: self.settings.is_tempered_affix_detection_enabled,
            aspect_detection_enabled: self.settings.rule_settings.aspect_detection_enabled,
            unique_detection_enabled: self.settings.rule_settings.unique_detection_enabled,
            rune_detection_enabled: self.settings.rule_settings.rune_detection_enabled,
            item_type,
            is_unique_item: marker_plan.is_unique_item,
            has_tooltip_top_splitter: marker_plan.has_tooltip_top_splitter,
        }
    }

    /// [`Self::execute_segment`] for every plan on up to `threads` threads.
    /// Each plan comes back with its own result, in plan order.
    fn execute_segments_parallel(
        &self,
        ocr_frame: &CaptureFrame,
        registry: &OcrEngineRegistry,
        plans: Vec<OcrSegmentPlan>,
        threads: usize,
    ) -> Vec<(OcrSegmentPlan, Result<SegmentResult, String>)> {
        let results = parallel_map(&plans, threads, |plan| {
            self.execute_segment(ocr_frame, registry, plan.clone())
        });
        plans.into_iter().zip(results).collect()
    }

    /// Crops, preprocesses, recognizes and converts one planned segment.
    ///
    /// The pixels handed to the OCR backend come from
    /// [`TooltipOcrPreprocessVariant`], and the image that was recognized is
    /// recorded as [`OcrInputTrace`] so the chain stays provable per segment.
    fn execute_segment(
        &self,
        ocr_frame: &CaptureFrame,
        registry: &OcrEngineRegistry,
        plan: OcrSegmentPlan,
    ) -> Result<SegmentResult, String> {
        let image = segment_ocr_image(ocr_frame, plan.rect, self.preprocess_variant)?;
        let ocr_input = ocr_input_trace(&image, self.preprocess_variant);
        let options = OcrOptions {
            segment_kind: plan.kind,
            page_seg_mode: plan.page_seg_mode,
            preserve_raw_text: true,
            source_region: Some(plan.rect),
            collect_tsv: true,
        };
        let start = Instant::now();
        // `recognize_structured` fails when the structured output cannot be
        // read; a failed TSV read never becomes a text-only success.
        let recognized = registry
            .recognize_structured(self.profile.clone(), &image, &options)
            .map_err(|error| error.to_string())?;
        let ocr_ms = elapsed_ms(start);
        Ok(self.convert_segment(plan, recognized.text, recognized.tsv, ocr_input, ocr_ms))
    }

    /// The upstream conversion of one recognized segment under `plan`.
    fn convert_segment(
        &self,
        plan: OcrSegmentPlan,
        text: OcrText,
        tsv: Option<String>,
        ocr_input: OcrInputTrace,
        ocr_ms: u128,
    ) -> SegmentResult {
        let conversion_start = Instant::now();
        let route = upstream_conversion_route(plan.kind, self.settings.is_socket_detection_enabled);
        let conversion_method = route.source_method;
        let converted_text = upstream_converted_text_for_route(route, &text.raw_text);
        let conversion_ms = elapsed_ms(conversion_start);
        let right_aligned_line_filtering_applied = plan.kind != OcrSegmentKind::UpperTooltip;
        SegmentResult {
            plan,
            text,
            tsv,
            ocr_input,
            converted_text,
            conversion_method,
            conversion_branch: route.branch,
            right_aligned_line_filtering_applied,
            type_area_offset_left: self.settings.type_area_offset_left,
            ocr_ms,
            conversion_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Core pipeline (called by both CLI read_tooltip and process_tooltip_ocr_parity)
// ---------------------------------------------------------------------------

/// Whether two plans hand the OCR backend the same pixels with the same
/// options: the same region, the same page segmentation mode, and the same
/// output parsing (the backend reads only whether the segment is the upper
/// tooltip section from the segment kind). Plans that differ only in their
/// conversion route (an affix area of a sigil) share one recognition.
fn same_recognition(left: &OcrSegmentPlan, right: &OcrSegmentPlan) -> bool {
    left.rect == right.rect
        && left.page_seg_mode == right.page_seg_mode
        && (left.kind == OcrSegmentKind::UpperTooltip)
            == (right.kind == OcrSegmentKind::UpperTooltip)
}

/// Mirrors upstream `ScreenProcessHandler.FindItemTypePower` ->
/// `OcrHandler.ConvertToItemType`: the item type is resolved from the OCR'd
/// upper tooltip section text (for sigils/runes: no item power line, last
/// lines fuzzy-matched against the item type index). Returns `None` when the
/// section text is missing, no session indexes are loaded, or no candidate
/// passed the `MinimalOcrMatchType` threshold. Upstream falls back to the
/// previous frame's item type in that case; the Rust runtime has no
/// cross-frame state and fails soft instead: unclassified affix segments
/// rather than an invented type.
fn upstream_item_type_from_upper_tooltip(
    raw_text: &str,
    fuzzy_indexes: Option<&FuzzyIndexes>,
) -> Option<String> {
    let indexes = fuzzy_indexes?;
    let conversion = convert_to_item_type(raw_text, indexes);
    conversion
        .type_id
        .filter(|type_id| !type_id.trim().is_empty())
}

/// The marker-derived segments recognized before the item type is known.
///
/// With rune detection off, a rune tooltip (known only after the upper
/// section is read) drops its affix areas (ScreenProcessHandler.cs:419), so
/// the first pass leaves the rune areas to the re-plan, which schedules them
/// only when the item type turns out not to be a rune.
fn first_pass_tail_plans(input: OcrSegmentPlanInput) -> Vec<OcrSegmentPlan> {
    let rune_detection_enabled = input.rune_detection_enabled;
    plan_ocr_segments(input)
        .into_iter()
        .filter(|plan| plan.kind != OcrSegmentKind::UpperTooltip)
        .filter(|plan| rune_detection_enabled || plan.kind != OcrSegmentKind::Rune)
        .collect()
}

/// Re-plans the marker-derived segments (everything except the upper tooltip
/// section, which is already OCR'd) with the item type resolved from the
/// upper tooltip text. Upstream `FindItemAffixes` consumes `ItemType` the
/// same way: a sigil tooltip converts every affix area with
/// `OcrHandler.ConvertToSigil`, a rune tooltip keeps only rune markers, and
/// charm/horadric-seal tooltips clear the affix areas entirely.
fn replan_tail_segments_with_resolved_item_type(
    segment_input: impl FnOnce(Option<String>) -> OcrSegmentPlanInput,
    upper_tooltip_raw_text: &str,
    fuzzy_indexes: Option<&FuzzyIndexes>,
) -> Vec<OcrSegmentPlan> {
    let item_type = upstream_item_type_from_upper_tooltip(upper_tooltip_raw_text, fuzzy_indexes);
    plan_ocr_segments(segment_input(item_type))
        .into_iter()
        .filter(|plan| plan.kind != OcrSegmentKind::UpperTooltip)
        .collect()
}

/// One-shot tooltip OCR: prepares session state (settings + templates) and
/// executes a single frame. Long-lived callers (the OCR worker) should
/// [`PreparedTooltipOcr::prepare`] once and call [`PreparedTooltipOcr::execute`]
/// per frame instead.
pub fn execute_tooltip_ocr_with_registry(
    root: &Path,
    frame: CaptureFrame,
    preset: String,
    profile: OcrProfile,
    registry: &OcrEngineRegistry,
) -> Result<TooltipOcrExecution, String> {
    PreparedTooltipOcr::prepare(root, preset, profile)?.execute(registry, &frame)
}

#[cfg(test)]
mod tests;
