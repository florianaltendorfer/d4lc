//! The live-aware stage, OCR and diagnostics panels of the Debug screen.

use std::rc::Rc;

use super::output::age_text;
use crate::app_state::{AppState, RuntimeStatus};
use crate::runtime_bridge::live_state::RECENT_DIAGNOSTICS;
use crate::slint_generated::AppWindow;

/// Project the live-aware stage and OCR panels.
///
/// Called when a new accepted snapshot (or the DebugInfo toggle) changed the
/// live debug data, and at every stored-run projection. It never touches the
/// artifact or timing models.
pub fn project_live_debug_stages(app: &AppWindow, state: &AppState) {
    app.set_debug_stage_rows(Rc::new(slint::VecModel::from(stage_rows(state))).into());
    app.set_debug_ocr_panels(Rc::new(slint::VecModel::from(ocr_panel_models(state))).into());
}

/// Project the live loop's newest diagnostics events.
pub fn project_live_diagnostics(app: &AppWindow, state: &AppState) {
    app.set_debug_diagnostics_rows(
        Rc::new(slint::VecModel::from(diagnostics_rows(
            &state.runtime_status.recent_diagnostics,
        )))
        .into(),
    );
}

/// The 8 processed-screen stage panels, live bundle first.
///
/// With an accepted live bundle each panel shows that stage's own bitmap,
/// labelled "Live, generation N". Without one it falls back to the newest
/// stored run's artifact and names that run; DebugInfo on-without-snapshot and
/// DebugInfo off are stated explicitly. Every panel therefore has exactly one
/// source and one provenance line — live and stored never mix.
fn stage_rows(state: &AppState) -> Vec<crate::slint_generated::DebugStageRow> {
    use crate::slint_generated as ui;

    let runtime = &state.runtime_status;
    let stored_panels = &state.debug().stage_panels;
    (0..8)
        .map(|index| {
            let stored = stored_panels.get(index);
            let label = stored
                .map(|panel| panel.label.clone())
                .unwrap_or_else(|| format!("Stage {index}"));

            if let Some(bundle) = runtime.live_debug_bundle.as_deref() {
                let source = live_bundle_source(runtime);
                return match live_stage_image(bundle, index) {
                    Some(image) => ui::DebugStageRow {
                        label: label.into(),
                        path: String::new().into(),
                        source: source.into(),
                        note: String::new().into(),
                        has_image: true,
                        image: debug_image_to_slint(image),
                    },
                    None => {
                        let note = format!("No {label} image in this accepted snapshot.");
                        ui::DebugStageRow {
                            label: label.into(),
                            path: String::new().into(),
                            source: source.into(),
                            note: note.into(),
                            has_image: false,
                            image: slint::Image::default(),
                        }
                    }
                };
            }

            // No live bundle: the stored run, with the live state named.
            let (path, note, has_image, image) = match stored {
                Some(panel) => {
                    let image = panel
                        .path
                        .as_deref()
                        .and_then(stage_image_from_link)
                        .unwrap_or_default();
                    let size = image.size();
                    (
                        panel.path.clone().unwrap_or_default(),
                        panel.note.clone(),
                        size.width > 0 && size.height > 0,
                        image,
                    )
                }
                None => (String::new(), String::new(), false, slint::Image::default()),
            };
            let source = stored_panel_source(runtime, &path);
            ui::DebugStageRow {
                label: label.into(),
                path: path.into(),
                source: source.into(),
                note: note.into(),
                has_image,
                image,
            }
        })
        .collect()
}

/// The provenance line of a stored-run stage panel: the run it came from plus
/// why the live bundle is not the source.
fn stored_panel_source(runtime: &RuntimeStatus, path: &str) -> String {
    let mut source = match &runtime.stored_debug_run {
        Some(run) => match run.captured_at {
            Some(at) => format!(
                "Stored run {}, captured {} ({}).",
                run.bundle_name,
                crate::clock::hh_mm_ss(at),
                age_text(at),
            ),
            None => format!(
                "Stored run {} (capture time not readable from the bundle name).",
                run.bundle_name,
            ),
        },
        None => "No stored run under debug-output/ yet.".to_string(),
    };
    if runtime.live_debug_info_enabled {
        source.push_str(" Debug information is on, but no accepted snapshot yet.");
    } else {
        source.push_str(" Debug information is off — live stages disabled.");
    }
    if path.is_empty() && runtime.stored_debug_run.is_none() {
        source.push_str(" Run Export debug images to capture one.");
    }
    source
}

/// The live stage bitmap of one panel index: 0 tooltip crop, 1 item-type
/// area, 2 affix locations, 3 affix areas, 4/5 aspect location/area (one
/// combined stage in this build's bundle), 6 socket locations, 7 splitter
/// locations. `None` when the accepted snapshot produced no such stage.
fn live_stage_image(
    bundle: &d4lc_runtime::TooltipDebugBundle,
    index: usize,
) -> Option<&d4lc_runtime::DebugImage> {
    match index {
        0 => Some(&bundle.tooltip_crop),
        1 => bundle.item_type_area.as_ref(),
        2 => Some(&bundle.affix_locations),
        3 => Some(&bundle.affix_areas),
        4 | 5 => Some(&bundle.aspect_area),
        6 => Some(&bundle.socket_locations),
        7 => Some(&bundle.splitter_locations),
        _ => None,
    }
}

/// One `DebugImage` as a Slint image. The bundle's RGBA buffer is tightly
/// packed and matches `Rgba8Pixel`, so the copy is the only conversion.
fn debug_image_to_slint(image: &d4lc_runtime::DebugImage) -> slint::Image {
    let width = image.width.max(1);
    let height = image.height.max(1);
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(image.rgba.len()) {
        // A malformed buffer is never rendered as garbage; the panel falls
        // back to its empty state.
        return slint::Image::default();
    }
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&image.rgba, width, height);
    slint::Image::from_rgba8(buffer)
}

/// The provenance line of the held debug bundle: "Live, generation N." while
/// the newest published state carries it, otherwise the held-over last
/// accepted snapshot with its generation and publish instant (upstream keeps
/// the last stage images on screen, `DebugViewModel.cs:466-500`; the label
/// keeps them from being read as the current frame).
fn live_bundle_source(runtime: &RuntimeStatus) -> String {
    let generation = runtime.live_debug_generation;
    if runtime.live_debug_bundle_current {
        return format!("Live, generation {generation}.");
    }
    let mut source = match runtime.live_debug_published_at {
        Some(at) => format!(
            "Last accepted snapshot, generation {generation}, published {} ({}); no accepted \
             snapshot is live now.",
            crate::clock::hh_mm_ss(at),
            age_text(at),
        ),
        None => format!(
            "Last accepted snapshot, generation {generation}; no accepted snapshot is live now."
        ),
    };
    if !runtime.live_debug_info_enabled {
        source.push_str(" Debug information is off.");
    }
    source
}

/// The 3 OCR result panels, live bundle lines first.
///
/// With an accepted live bundle the panels list the exact OCR text lines the
/// snapshot recognized (segment index, text, segment kind). Without one they
/// fall back to the stored/parse content `actions::debug` builds; the source
/// line names which, so the two never mix.
fn ocr_panel_models(state: &AppState) -> Vec<crate::slint_generated::DebugOcrPanel> {
    use crate::slint_generated as ui;

    let runtime = &state.runtime_status;
    if let Some(bundle) = runtime.live_debug_bundle.as_deref() {
        let source = live_bundle_source(runtime);
        let kinds: [(String, &[&str]); 3] = [
            (
                crate::i18n::tr("OCR Results (Affixes)"),
                &["affix", "rune", "sigil"],
            ),
            (
                crate::i18n::tr("OCR Results (Aspect)"),
                &["aspect", "unique_aspect"],
            ),
            (
                crate::i18n::tr("OCR Results (ItemType + Power)"),
                &["upper_tooltip"],
            ),
        ];
        return kinds
            .iter()
            .map(|(title, wanted)| {
                let rows: Vec<ui::DebugOcrRow> = bundle
                    .lines
                    .iter()
                    .filter(|line| wanted.contains(&line.segment_kind))
                    .map(|line| ui::DebugOcrRow {
                        id: line.segment_index.to_string().into(),
                        text: line.text.clone().into(),
                        source: line.segment_kind.into(),
                    })
                    .collect();
                let note = if rows.is_empty() {
                    "No recognized text line of this panel's segments in the accepted snapshot."
                        .to_string()
                } else {
                    format!(
                        "Live OCR text lines of the accepted snapshot ({} line(s)); upstream \
                         additionally shows the cleaned text and the parsed value columns.",
                        rows.len()
                    )
                };
                ui::DebugOcrPanel {
                    title: title.as_str().into(),
                    source: source.clone().into(),
                    note: note.into(),
                    columns: Rc::new(slint::VecModel::from(
                        [
                            "Segment".to_string(),
                            crate::i18n::tr("Text"),
                            "Kind".to_string(),
                        ]
                        .into_iter()
                        .map(slint::SharedString::from)
                        .collect::<Vec<_>>(),
                    ))
                    .into(),
                    rows: Rc::new(slint::VecModel::from(rows)).into(),
                }
            })
            .collect();
    }

    let source = stored_ocr_source(runtime);
    state
        .debug()
        .ocr_panels
        .iter()
        .map(|panel| ui::DebugOcrPanel {
            title: panel.title.clone().into(),
            source: source.clone().into(),
            note: panel.note.clone().into(),
            columns: Rc::new(slint::VecModel::from(
                panel
                    .columns
                    .iter()
                    .map(|column| slint::SharedString::from(column.as_str()))
                    .collect::<Vec<_>>(),
            ))
            .into(),
            rows: Rc::new(slint::VecModel::from(
                panel
                    .rows
                    .iter()
                    .map(|row| ui::DebugOcrRow {
                        id: row.first().cloned().unwrap_or_default().into(),
                        text: row.get(1).cloned().unwrap_or_default().into(),
                        source: row.get(2).cloned().unwrap_or_default().into(),
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        })
        .collect()
}

/// The provenance line of an OCR panel without a live bundle: the live parse
/// state the port renders, with the live DebugInfo state named.
fn stored_ocr_source(runtime: &RuntimeStatus) -> String {
    if runtime.live_debug_info_enabled {
        "Live parse state (no accepted snapshot yet; Debug information is on).".to_string()
    } else {
        "Live parse state of the current frame (Debug information is off; enable it for the raw OCR lines)."
            .to_string()
    }
}

/// The newest diagnostics events (bounded, newest first) as panel rows.
fn diagnostics_rows(
    events: &[d4lc_runtime::DiagnosticsEvent],
) -> Vec<crate::slint_generated::DebugDiagnosticRow> {
    use crate::slint_generated as ui;

    events
        .iter()
        .take(RECENT_DIAGNOSTICS)
        .map(|event| {
            let (outcome, reason) = match &event.outcome {
                d4lc_runtime::DiagnosticsOutcome::Ok => ("ok", String::new()),
                d4lc_runtime::DiagnosticsOutcome::Failed { message } => ("failed", message.clone()),
                d4lc_runtime::DiagnosticsOutcome::Discarded { reason } => {
                    ("discarded", reason.as_str().to_string())
                }
            };
            ui::DebugDiagnosticRow {
                phase: event.phase.as_str().into(),
                outcome: outcome.into(),
                reason: reason.into(),
                generation: i32::try_from(event.tooltip_generation).unwrap_or(i32::MAX),
            }
        })
        .collect()
}

/// Load a stage image from its artifact-root-relative link. A failed decode
/// keeps the panel in its text state (the note still names the file).
fn stage_image_from_link(link: &str) -> Option<slint::Image> {
    let path = crate::actions::debug::screenshot::artifact_root().join(link);
    let image = slint::Image::load_from_path(&path).ok()?;
    let size = image.size();
    (size.width > 0 && size.height > 0).then_some(image)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::actions::debug::test_support::fresh_state;

    /// One tiny RGBA stage bitmap.
    fn debug_image(width: u32, height: u32, red: u8) -> d4lc_runtime::DebugImage {
        let mut rgba = vec![0_u8; (width * height * 4) as usize];
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[0] = red;
            pixel[3] = 255;
        }
        d4lc_runtime::DebugImage {
            width,
            height,
            rgba: std::sync::Arc::from(rgba),
        }
    }

    /// An accepted live debug bundle with every stage and two text lines.
    fn live_bundle() -> d4lc_runtime::TooltipDebugBundle {
        let crop = debug_image(4, 4, 10);
        let area = debug_image(2, 2, 20);
        d4lc_runtime::TooltipDebugBundle {
            tooltip_crop: crop.clone(),
            item_type_area: Some(area),
            affix_locations: crop.clone(),
            affix_areas: crop.clone(),
            aspect_area: crop.clone(),
            socket_locations: crop.clone(),
            splitter_locations: crop.clone(),
            segments: Vec::new(),
            lines: vec![d4lc_runtime::DebugTextLine {
                segment_index: 0,
                segment_kind: "affix",
                text: "+10 Strength".to_string(),
                rect: d4lc_core::Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
            }],
        }
    }

    /// With a live bundle every stage panel shows the bundle's own bitmap and
    /// names its generation; the OCR panel lists the recognized line.
    #[test]
    fn live_stage_rows_name_the_generation_and_use_the_bundle_images() {
        let mut state = fresh_state();
        state.runtime_status.live_debug_info_enabled = true;
        state.runtime_status.live_debug_generation = 7;
        state.runtime_status.live_debug_bundle = Some(std::sync::Arc::new(live_bundle()));
        state.runtime_status.live_debug_bundle_current = true;

        let rows = stage_rows(&state);
        assert_eq!(rows.len(), 8, "one row per upstream stage panel");
        for (index, row) in rows.iter().enumerate() {
            assert!(row.has_image, "stage {index} carries the live bitmap");
            assert_eq!(row.source.as_str(), "Live, generation 7.");
            assert!(row.path.is_empty(), "a live panel carries no artifact path");
        }

        let panels = ocr_panel_models(&state);
        assert_eq!(panels.len(), 3);
        assert_eq!(panels[0].source.as_str(), "Live, generation 7.");
        use slint::Model as _;
        assert_eq!(panels[0].rows.row_count(), 1);
        assert_eq!(
            panels[0]
                .rows
                .row_data(0)
                .map(|row| row.text.to_string())
                .as_deref(),
            Some("+10 Strength")
        );
        assert_eq!(
            panels[1].rows.row_count(),
            0,
            "no aspect line in the bundle"
        );
    }

    /// A bundle held over from an earlier accepted snapshot keeps its images
    /// but is labelled as the last accepted snapshot, never as live.
    #[test]
    fn held_bundle_keeps_its_images_and_is_labelled_as_the_last_snapshot() {
        let mut state = fresh_state();
        state.runtime_status.live_debug_info_enabled = true;
        state.runtime_status.live_debug_generation = 7;
        state.runtime_status.live_debug_bundle = Some(std::sync::Arc::new(live_bundle()));
        state.runtime_status.live_debug_bundle_current = false;

        let rows = stage_rows(&state);
        for row in &rows {
            assert!(row.has_image, "the held bundle keeps its bitmap");
            assert!(
                row.source
                    .starts_with("Last accepted snapshot, generation 7")
                    && row.source.contains("no accepted snapshot is live now"),
                "held provenance: {}",
                row.source
            );
            assert!(!row.source.contains("Live, generation"));
        }
        assert!(ocr_panel_models(&state)[0]
            .source
            .starts_with("Last accepted snapshot, generation 7"));

        state.runtime_status.live_debug_info_enabled = false;
        assert!(stage_rows(&state)[0]
            .source
            .ends_with("Debug information is off."));
    }

    /// Without a live bundle the stage panels name the stored run and the
    /// DebugInfo state; live and stored content never mix.
    #[test]
    fn stage_rows_name_the_stored_run_and_the_off_state() {
        let mut state = fresh_state();
        let rows = stage_rows(&state);
        assert_eq!(rows.len(), 8);
        for row in &rows {
            assert!(!row.has_image);
            assert!(
                row.source.contains("Debug information is off"),
                "the off state must be named: {}",
                row.source
            );
            assert!(row
                .source
                .contains("No stored run under debug-output/ yet."));
        }

        // DebugInfo on but no accepted snapshot yet: the enabled-but-empty
        // state is named, not silently the off state.
        state.runtime_status.live_debug_info_enabled = true;
        let rows = stage_rows(&state);
        assert!(rows[0].source.contains("no accepted snapshot yet"));
    }

    /// The diagnostics event list maps each event to its phase, outcome,
    /// reason and generation, keeps only the bound and never synthesizes a
    /// reason.
    #[test]
    fn diagnostics_rows_map_the_events_and_stay_bounded() {
        let make = |frame: u64, phase, outcome| d4lc_runtime::DiagnosticsEvent {
            session_id: 1,
            frame_id: frame,
            tooltip_generation: frame,
            config_revision: d4lc_runtime::Revision::Applied(1),
            rules_revision: d4lc_runtime::Revision::Unknown,
            phase,
            at_micros: frame,
            wall_unix_ms: 1_700_000_000_000,
            duration_micros: None,
            first_seen_at_micros: None,
            outcome,
            detail: None,
        };
        // Newest first, longer than the bound: the newest event failed, the
        // next was discarded, the rest are ok.
        let events: Vec<_> = (0..u64::try_from(RECENT_DIAGNOSTICS).expect("bound") + 3)
            .rev()
            .map(|frame| {
                let outcome = match frame {
                    22 => d4lc_runtime::DiagnosticsOutcome::failed("boom"),
                    21 => d4lc_runtime::DiagnosticsOutcome::discarded(
                        d4lc_runtime::DiscardReason::Superseded,
                    ),
                    _ => d4lc_runtime::DiagnosticsOutcome::Ok,
                };
                make(frame, d4lc_runtime::DiagnosticsPhase::OcrSubmit, outcome)
            })
            .collect();

        let rows = diagnostics_rows(&events);
        assert_eq!(rows.len(), RECENT_DIAGNOSTICS, "the list is bounded");
        assert_eq!(rows[0].generation, 22, "newest first");
        assert_eq!(rows[0].outcome.as_str(), "failed");
        assert_eq!(rows[0].reason.as_str(), "boom");
        assert_eq!(rows[1].outcome.as_str(), "discarded");
        assert_eq!(rows[1].reason.as_str(), "Superseded");
        assert_eq!(rows[2].outcome.as_str(), "ok");
        assert!(rows[2].reason.is_empty(), "an ok event carries no reason");
        assert_eq!(rows[2].generation, 20);
    }
}
