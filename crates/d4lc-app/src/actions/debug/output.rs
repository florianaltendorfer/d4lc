//! The `debug-output/` scan and the stored-run panel projection.

use std::path::Path;
use std::rc::Rc;

use super::live::{project_live_debug_stages, project_live_diagnostics};
use crate::app_state::{AppState, StoredDebugRun};
use crate::slint_generated::AppWindow;

/// Project the debug panel state (artifact links, stage panels, perf
/// table, OCR panels) from the debug-output directory into the screen.
///
/// Upstream binds live bitmaps and OCR collections
/// (`DebugViewModel.cs:240-318,462-571`); the port's equivalent source is
/// the artifact bundle this build writes under
/// `debug-output/`. Every value is
/// read from a real file or a real runtime/parse value; a stage or
/// artifact the bundle does not carry renders as a missing state with the
/// reason, never as a synthesized stand-in.
pub fn project_debug_panels(app: &AppWindow, state: &mut AppState, debug_dir: &Path) {
    let scan = scan_debug_output(debug_dir);
    *state.debug_mut() = build_debug_panel_state(state, &scan);
    state.runtime_status.stored_debug_run = stored_debug_run(&scan);
    project_debug_panels_to_ui(app, state, &scan);
}

/// The stored run the debug panels fall back to when no live bundle exists:
/// the newest export bundle's name and the capture instant its own name
/// carries. `None` without a bundle.
fn stored_debug_run(scan: &DebugOutputScan) -> Option<StoredDebugRun> {
    let (name, _) = scan.bundle.as_ref()?;
    Some(StoredDebugRun {
        bundle_name: name.clone(),
        captured_at: scan.bundle_captured_at(),
    })
}

/// The panel state one debug-output directory produces.
///
/// Split from [`project_debug_panels`] so the projection can be exercised
/// without a Slint window (the screen projection only writes the models
/// this state already carries).
#[cfg(test)]
pub fn debug_panel_state_for_dir(state: &AppState, debug_dir: &Path) -> d4lc_gui::DebugPanelState {
    build_debug_panel_state(state, &scan_debug_output(debug_dir))
}

fn build_debug_panel_state(state: &AppState, scan: &DebugOutputScan) -> d4lc_gui::DebugPanelState {
    d4lc_gui::DebugPanelState::from_fixture(d4lc_gui::DebugPanelFixture {
        artifact_links: bundle_artifact_links(scan),
        active_language: state.shell.settings.language.clone(),
        overlay_status: state.debug().overlay_status.clone(),
        timing_metrics: state.debug().timing_metrics.clone(),
        debug_info_enabled: state.debug().debug_info_enabled,
        stage_panels: stage_panels(scan),
        perf_rows: perf_rows(scan),
        ocr_panels: ocr_panels(state),
    })
}

/// Debug-output directory scan: the newest export bundle plus the newest
/// screenshot, both by their `<unix>` name suffix (deterministic, no
/// filesystem timestamp dependence).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DebugOutputScan {
    dir: std::path::PathBuf,
    bundle: Option<(String, std::path::PathBuf)>,
    screenshot: Option<(String, std::path::PathBuf)>,
}

impl DebugOutputScan {
    fn bundle_name(&self) -> Option<&str> {
        self.bundle.as_ref().map(|(name, _)| name.as_str())
    }

    /// When the newest bundle was captured, from its own name — the
    /// `export-<unix seconds>` suffix this build writes (deterministic, no
    /// filesystem-timestamp dependence). `None` for a name without a readable
    /// suffix: the age is then simply not claimed.
    fn bundle_captured_at(&self) -> Option<std::time::SystemTime> {
        let suffix = self.bundle_name()?.strip_prefix("export-")?;
        let seconds = parse_unix_suffix(suffix)?;
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
    }
}

fn scan_debug_output(debug_dir: &Path) -> DebugOutputScan {
    let mut scan = DebugOutputScan {
        dir: debug_dir.to_path_buf(),
        ..DebugOutputScan::default()
    };
    let Ok(entries) = std::fs::read_dir(debug_dir) else {
        return scan;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() {
            if let Some(suffix) = name.strip_prefix("export-").and_then(parse_unix_suffix) {
                let newer = scan
                    .bundle
                    .as_ref()
                    .and_then(|(_, path)| {
                        path.file_name().and_then(|n| {
                            n.to_string_lossy()
                                .strip_prefix("export-")
                                .map(str::to_string)
                        })
                    })
                    .and_then(|n| parse_unix_suffix(&n))
                    .is_none_or(|current| suffix > current);
                if newer {
                    scan.bundle = Some((name, entry.path()));
                }
            }
        } else if let Some(suffix) = name
            .strip_prefix("screenshot-")
            .and_then(|rest| rest.strip_suffix(".png"))
            .and_then(parse_unix_suffix)
        {
            let newer = scan
                .screenshot
                .as_ref()
                .and_then(|(n, _)| {
                    n.strip_prefix("screenshot-")
                        .and_then(|rest| rest.strip_suffix(".png"))
                        .and_then(parse_unix_suffix)
                })
                .is_none_or(|current| suffix > current);
            if newer {
                scan.screenshot = Some((name, entry.path()));
            }
        }
    }
    scan
}

/// A directory mtime this recent may still hide a second change within the
/// same timestamp tick (coarse-granularity filesystems: FAT keeps 2 s), so
/// such a stamp never vouches for the cached scan.
const DEBUG_OUTPUT_RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// Periodic `debug-output/` scan that re-reads the directory only when its
/// own mtime changed: one `metadata` call per poll instead of a
/// `read_dir` plus one stat per entry.
///
/// The scan result depends only on the directory's entry names and kinds,
/// and adding, removing or renaming an entry updates the directory's mtime.
/// A stamp younger than [`DEBUG_OUTPUT_RACY_WINDOW`] (or in the future), an
/// unreadable stamp next to an existing directory, or another directory
/// always rescans, so the cached result equals a fresh scan.
#[derive(Debug, Default)]
pub struct DebugOutputWatcher {
    /// The last scan and the stamp that vouches for it; `None` when the
    /// stamp was not trusted at scan time (a change later in the same tick
    /// would leave the stamp unchanged), so that scan is never reused.
    cached: Option<(Option<Option<std::time::SystemTime>>, DebugOutputScan)>,
    /// Directory reads performed (for the tests).
    reads: u64,
}

impl DebugOutputWatcher {
    fn scan(&mut self, debug_dir: &Path) -> &DebugOutputScan {
        self.scan_at(debug_dir, std::time::SystemTime::now())
    }

    fn scan_at(&mut self, debug_dir: &Path, now: std::time::SystemTime) -> &DebugOutputScan {
        let stamp = std::fs::metadata(debug_dir).map(|metadata| metadata.modified());
        let stamp = match stamp {
            // No directory: a fresh scan is empty too; `None` is its stamp.
            Err(_) => Some(None),
            Ok(Ok(modified)) => Some(Some(modified)),
            // A directory without a readable mtime cannot vouch for a cache.
            Ok(Err(_)) => None,
        };
        let trusted = stamp.filter(|stamp| {
            stamp.is_none_or(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age >= DEBUG_OUTPUT_RACY_WINDOW)
            })
        });
        let reusable = matches!(
            (&self.cached, trusted),
            (Some((Some(cached_stamp), scan)), Some(stamp))
                if *cached_stamp == stamp && scan.dir == debug_dir
        );
        if !reusable {
            self.cached = None;
        }
        let reads = &mut self.reads;
        let (_, scan) = self.cached.get_or_insert_with(|| {
            *reads += 1;
            (trusted, scan_debug_output(debug_dir))
        });
        scan
    }
}

fn parse_unix_suffix(value: &str) -> Option<u64> {
    value.parse::<u64>().ok()
}

/// The bundle file → artifact kind table. Only files this build really
/// writes are mapped; a kind without a file stays a missing state.
const BUNDLE_ARTIFACTS: [(d4lc_gui::DebugArtifactKind, &str); 9] = [
    (d4lc_gui::DebugArtifactKind::RawCapture, "raw-capture.png"),
    (d4lc_gui::DebugArtifactKind::RoiCrop, "roi.png"),
    (d4lc_gui::DebugArtifactKind::TooltipCrop, "tooltip-crop.png"),
    (
        d4lc_gui::DebugArtifactKind::ThresholdMask,
        "tooltip-crop-thresholded.png",
    ),
    (
        d4lc_gui::DebugArtifactKind::TemplateTrace,
        "detect-trace.json",
    ),
    (
        d4lc_gui::DebugArtifactKind::ParsedItemJson,
        "parsed-item.json",
    ),
    (
        d4lc_gui::DebugArtifactKind::RuleEvaluationJson,
        "rule-result.json",
    ),
    (
        d4lc_gui::DebugArtifactKind::OverlayStatus,
        "overlay-status.json",
    ),
    (d4lc_gui::DebugArtifactKind::Timings, "timings.json"),
];

/// Artifact links for the newest bundle plus the newest screenshot.
///
/// `DebugArtifactState` only counts paths inside `debug-output/` as
/// present, so links are workspace-relative when the debug directory sits
/// inside the checkout (the normal case) and absolute otherwise (the
/// deployed-binary temp fallback) — the missing state then reflects the
/// port's artifact contract.
fn bundle_artifact_links(scan: &DebugOutputScan) -> Vec<d4lc_gui::DebugArtifactLink> {
    let mut links = Vec::new();
    if let Some((_, bundle_dir)) = &scan.bundle {
        for (kind, file_name) in BUNDLE_ARTIFACTS {
            let path = bundle_dir.join(file_name);
            if path.is_file() {
                links.push(d4lc_gui::DebugArtifactLink::new(
                    kind,
                    &debug_link_path(&path),
                ));
            }
        }
    }
    if let Some((_, screenshot_path)) = &scan.screenshot {
        // A standalone screenshot is a raw capture; when the bundle also
        // carries one, the bundle copy is the richer artifact.
        if !links
            .iter()
            .any(|link| link.kind == d4lc_gui::DebugArtifactKind::RawCapture)
        {
            links.push(d4lc_gui::DebugArtifactLink::new(
                d4lc_gui::DebugArtifactKind::RawCapture,
                &debug_link_path(screenshot_path),
            ));
        }
    }
    links
}

fn debug_link_path(path: &Path) -> String {
    let root = crate::actions::debug::screenshot::artifact_root();
    debug_link_path_in(Some(&root), path)
}

/// Link for one debug-output file, relative to the artifact root.
///
/// The artifact contract (`is_debug_output_path`) counts only paths under
/// `debug-output/` as present; a file outside the root keeps its absolute
/// path.
fn debug_link_path_in(root: Option<&Path>, path: &Path) -> String {
    root.and_then(|root| path.strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.display().to_string())
}

/// The 8 processed-screen stage panels (`DebugView.xaml:200-247`).
///
/// The port's debug bundle carries the processed tooltip crop; it does
/// not write a per-stage image for the marker/area stages upstream draws
/// from its live pipeline, so those panels state the absence instead of
/// rendering a stand-in.
fn stage_panels(scan: &DebugOutputScan) -> Vec<d4lc_gui::DebugStagePanel> {
    const STAGES: [&str; 8] = [
        "ItemTooltip",
        "ItemType",
        "ItemAffixesLocations",
        "ItemAffixesAreas",
        "ItemAspectLocation",
        "ItemAspectArea",
        "ItemSocketLocations",
        "ItemSplitterLocations",
    ];
    let tooltip_crop = scan
        .bundle
        .as_ref()
        .map(|(name, dir)| (name.clone(), dir.join("tooltip-crop.png")))
        .filter(|(_, path)| path.is_file());

    STAGES
        .iter()
        .map(|stage| {
            let path = match (*stage, &tooltip_crop) {
                ("ItemTooltip", Some((_, crop))) => Some(debug_link_path(crop)),
                _ => None,
            };
            let note = if path.is_some() {
                "Processed tooltip crop from the debug bundle (upstream: live \
                 ProcessedScreenItemTooltip)."
                    .to_string()
            } else {
                format!(
                    "No {stage} artifact in this build's debug bundle — the port writes the raw \
                     capture, ROI crop, tooltip crop, threshold mask and detection trace; upstream \
                     renders this stage from its live pipeline."
                )
            };
            d4lc_gui::DebugStagePanel {
                label: (*stage).to_string(),
                path,
                note,
            }
        })
        .collect()
}

/// The per-stage timing rows (`DebugView.xaml:445-462`).
///
/// Upstream plots 11 LiveCharts series; the port renders the 11 detect
/// timings its bundle's `timings.json` really carries. Without a bundle
/// the rows stay visible with no measurement (rendered as "—").
fn perf_rows(scan: &DebugOutputScan) -> Vec<d4lc_gui::DebugPerfRow> {
    const SERIES: [(&str, &str); 11] = [
        ("asset_load_ms", "Asset load"),
        ("roi_crop_ms", "ROI crop"),
        ("grayscale_ms", "Grayscale"),
        ("template_select_ms", "Template select"),
        ("template_load_ms", "Template load"),
        ("template_prepare_ms", "Template prepare"),
        ("match_ms", "Template match"),
        ("coordinate_translate_ms", "Coordinate translate"),
        ("trace_write_ms", "Trace write"),
        ("crop_write_ms", "Crop write"),
        ("total_ms", "Total"),
    ];
    let timings = scan
        .bundle
        .as_ref()
        .map(|(_, dir)| dir.join("timings.json"))
        .filter(|path| path.is_file())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());

    SERIES
        .iter()
        .map(|(key, label)| d4lc_gui::DebugPerfRow {
            name: (*label).to_string(),
            value_ms: timings
                .as_ref()
                .and_then(|timings| detect_timing_ms(timings, key)),
        })
        .collect()
}

/// One `detect` stage timing from a bundle's `timings.json` (the shape
/// `actions/debug/export.rs` writes). Returns `None` when the key is absent
/// or its value is not a number, so an unmeasured stage stays unmeasured.
fn detect_timing_ms(timings: &serde_json::Value, key: &str) -> Option<f64> {
    timings.get("detect")?.get(key)?.as_f64()
}

/// The 3 OCR result panels (`DebugView.xaml:249-419`).
///
/// The rows are the live parse values this build really has
/// (`AppState::current_item`); the raw OCR text of the runtime's OCR
/// worker is not exposed to the GUI, so each panel states its source and
/// the upstream column difference instead of inventing OCR text.
fn ocr_panels(state: &AppState) -> Vec<d4lc_gui::DebugOcrPanel> {
    let item = state.current_item();

    let mut affix_rows: Vec<Vec<String>> = Vec::new();
    for affix in &item.affixes {
        affix_rows.push(vec![
            affix.affix_id.clone().unwrap_or_else(|| "-".to_string()),
            affix.label.clone(),
            affix.source_text.clone(),
        ]);
    }
    let affix_note = if item.affixes.is_empty() {
        "No parsed affix rows yet — start the live loop and let it parse a tooltip.".to_string()
    } else {
        format!(
            "Live parsed affix rows for the current frame. Upstream lists the raw OCR text, the \
             cleaned text and the OCR value columns; the port's runtime OCR text stays inside the \
             OCR worker and is not exposed to the GUI ({} row(s)).",
            item.affixes.len()
        )
    };

    let aspect_rows: Vec<Vec<String>> = item
        .non_affix_rows
        .iter()
        .map(|row| {
            vec![
                row.id.clone().unwrap_or_else(|| "-".to_string()),
                row.label.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    let aspect_note = if aspect_rows.is_empty() {
        "No classified aspect/unique rows for the current frame.".to_string()
    } else {
        format!(
            "Live classified non-affix rows (aspect, unique, rune, sigil) for the current frame; \
             upstream shows a single aspect OCR card with its raw and cleaned text ({} row(s)).",
            aspect_rows.len()
        )
    };

    let type_rows: Vec<Vec<String>> = vec![
        vec![
            "Item".to_string(),
            item.item_name.clone().unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "Type".to_string(),
            item.item_type.clone().unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "Power".to_string(),
            item.item_power
                .map(|power| power.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "Rarity".to_string(),
            item.rarity.clone().unwrap_or_else(|| "-".to_string()),
        ],
    ];
    let type_note = format!(
        "Live parsed item fields for the current frame (parse status '{}'); upstream shows the \
         type-match similarity line and the OCR power text.",
        item.parse_status
    );

    vec![
        d4lc_gui::DebugOcrPanel {
            title: crate::i18n::tr("OCR Results (Affixes)"),
            columns: vec![
                crate::i18n::tr("Id"),
                crate::i18n::tr("Text"),
                "Source".to_string(),
            ],
            rows: affix_rows,
            note: affix_note,
        },
        d4lc_gui::DebugOcrPanel {
            title: crate::i18n::tr("OCR Results (Aspect)"),
            columns: vec![crate::i18n::tr("Id"), crate::i18n::tr("Text")],
            rows: aspect_rows,
            note: aspect_note,
        },
        d4lc_gui::DebugOcrPanel {
            title: crate::i18n::tr("OCR Results (ItemType + Power)"),
            columns: vec!["Field".to_string(), "Value".to_string()],
            rows: type_rows,
            note: type_note,
        },
    ]
}

/// Project the stored run's per-stage timing table (rows + source note).
pub(super) fn project_perf_rows_to_ui(app: &AppWindow, state: &AppState, source: &str) {
    use crate::slint_generated as ui;

    let perf: Vec<ui::KeyValueRow> = state
        .debug()
        .perf_rows
        .iter()
        .map(|row| ui::KeyValueRow {
            key: row.name.clone().into(),
            value: row
                .value_ms
                .map(|value| format!("{value:.2} ms"))
                .unwrap_or_else(|| "—".to_string())
                .into(),
        })
        .collect();
    app.set_debug_perf_rows(Rc::new(slint::VecModel::from(perf)).into());
    app.set_debug_perf_source(source.into());
}

/// Project the debug panel state into the screen's models + source notes.
fn project_debug_panels_to_ui(app: &AppWindow, state: &AppState, scan: &DebugOutputScan) {
    use crate::slint_generated as ui;

    let artifacts: Vec<ui::DebugArtifactRow> = state
        .debug()
        .artifacts
        .iter()
        .map(|artifact| ui::DebugArtifactRow {
            label: artifact.label.clone().into(),
            path: artifact.path.clone().unwrap_or_default().into(),
            present: artifact.present,
        })
        .collect();
    app.set_debug_artifact_rows(Rc::new(slint::VecModel::from(artifacts)).into());
    app.set_debug_artifact_source(artifact_source_text(scan).into());

    project_perf_rows_to_ui(app, state, &perf_source_text(scan));
    project_live_debug_stages(app, state);
    project_live_diagnostics(app, state);
}

fn artifact_source_text(scan: &DebugOutputScan) -> String {
    match (&scan.bundle, &scan.screenshot) {
        (Some((name, _)), _) => {
            format!("Newest debug bundle: {name} (screenshot/export actions refresh this list)")
        }
        (None, Some((name, _))) => {
            format!("Newest screenshot: {name} (no export bundle yet)")
        }
        (None, None) => format!(
            "No debug artifacts yet under {} — use Take screenshot or Export debug images.",
            scan.dir.display()
        ),
    }
}

/// The stored-run provenance line above the per-stage timing table.
///
/// The table reads a previous export's `timings.json`, so the line always
/// names the run it came from and how old it is — never a bare "timings" next
/// to the live frame numbers above. Without a stored run it says so and names
/// the action that produces one.
fn perf_source_text(scan: &DebugOutputScan) -> String {
    let Some(name) = scan.bundle_name() else {
        return "No stored run yet: run Export debug images to capture per-stage timings. The live \
                values above come from the running loop, not from this table."
            .to_string();
    };
    match scan.bundle_captured_at() {
        Some(captured_at) => format!(
            "Stored run {name}/timings.json, captured {} ({}): per-stage detection timings of that \
             export, not the live frame above.",
            crate::clock::hh_mm_ss(captured_at),
            age_text(captured_at),
        ),
        None => format!(
            "Stored run {name}/timings.json (capture time not readable from the bundle name): \
             per-stage detection timings of that export, not the live frame above."
        ),
    }
}

/// How long ago `at` was, in whole minutes/hours/days.
pub(super) fn age_text(at: std::time::SystemTime) -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(at)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    match elapsed {
        0..=59 => format!("{elapsed} s ago"),
        60..=3599 => format!("{} min ago", elapsed / 60),
        3600..=86_399 => format!("{} h {} min ago", elapsed / 3600, (elapsed % 3600) / 60),
        _ => format!("{} d ago", elapsed / 86_400),
    }
}

/// Re-render the stored-run provenance line when its age moved on.
///
/// The table's rows are a stored run's numbers; only the "how old is it" part
/// changes with time. Called from the Debug screen's periodic refresh while
/// the tab is visible, so the age is never a stale claim.
pub fn refresh_stored_run_note(
    app: &AppWindow,
    watcher: &mut DebugOutputWatcher,
    debug_dir: &Path,
) {
    let source = perf_source_text(watcher.scan(debug_dir));
    if app.get_debug_perf_source() != source.as_str() {
        app.set_debug_perf_source(source.into());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::actions::debug::test_support::{debug_test_root, fresh_state};

    /// The debug-output scan + panel projection: a bundle with real files
    /// produces artifact links, the tooltip stage image, the 11 timing
    /// rows and missing states for everything this build does not
    /// write.
    #[test]
    fn panel_projection_reads_the_bundle_and_marks_missing_artifacts() {
        let root = debug_test_root("panels");
        let dir = root.join("debug-output");
        let bundle = dir.join("export-1700000000");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("raw-capture.png"), b"png").unwrap();
        std::fs::write(bundle.join("tooltip-crop.png"), b"png").unwrap();
        std::fs::write(bundle.join("overlay-status.json"), "{}").unwrap();
        std::fs::write(
            bundle.join("timings.json"),
            "{\n  \"detect\": {\n    \"match_ms\": 4.25,\n    \"total_ms\": 9.5\n  }\n}\n",
        )
        .unwrap();
        // An older bundle must not win over the newer one.
        let older = dir.join("export-1699999999");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::write(older.join("raw-capture.png"), b"png").unwrap();

        let mut state = fresh_state();
        state.shell.settings.language = "enUS".to_string();
        state
            .current_item_mut()
            .affixes
            .push(d4lc_gui::CurrentItemAffixView {
                row_id: d4lc_core::AffixRowId(0),
                affix_id: Some("Affix_AttackSpeed".to_string()),
                label: "Attack Speed".to_string(),
                value_text: Some("18.0%".to_string()),
                source_text: "+18.0% Attack Speed".to_string(),
                is_greater: true,
                is_implicit: false,
                is_tempered: false,
                matched_by_rule: false,
            });

        let panel = debug_panel_state_for_dir(&state, &dir);

        let artifact = |kind| {
            panel
                .artifact(kind)
                .unwrap_or_else(|| panic!("artifact {kind:?} must be projected"))
        };
        // The scan picks the newest bundle and maps only the files it
        // really contains (links are the raw projection; the gui drops
        // links outside `debug-output/`).
        let scan = scan_debug_output(&dir);
        assert_eq!(scan.bundle_name(), Some("export-1700000000"));
        let links = bundle_artifact_links(&scan);
        let link = |kind| {
            links
                .iter()
                .find(|link| link.kind == kind)
                .unwrap_or_else(|| panic!("link {kind:?} must be projected"))
        };
        assert!(link(d4lc_gui::DebugArtifactKind::RawCapture)
            .path
            .contains("export-1700000000"));
        assert!(link(d4lc_gui::DebugArtifactKind::TooltipCrop)
            .path
            .ends_with("tooltip-crop.png"));
        assert!(link(d4lc_gui::DebugArtifactKind::OverlayStatus)
            .path
            .ends_with("overlay-status.json"));
        assert!(links
            .iter()
            .all(|link| link.kind != d4lc_gui::DebugArtifactKind::OcrText));
        assert!(links
            .iter()
            .all(|link| link.kind != d4lc_gui::DebugArtifactKind::ParsedItemJson));
        // This test's debug directory lives outside the checkout, so the
        // `debug-output/` artifact contract reports every kind as a
        // missing state (the workspace-relative case is pinned by
        // `debug_link_path_prefers_workspace_relative_paths`).
        assert_eq!(panel.debug_artifact_count, 0);
        assert!(artifact(d4lc_gui::DebugArtifactKind::RawCapture)
            .path
            .is_none());

        // Stage panels: the tooltip crop is the one stage image the
        // bundle carries; the other seven state the absence.
        assert_eq!(panel.stage_panels.len(), 8);
        assert!(panel.stage_panels[0].path.is_some());
        assert!(panel.stage_panels[1].path.is_none());
        assert!(panel.stage_panels[1].note.contains("No ItemType artifact"));

        // The 11 timing series render with their measured values; an
        // unmeasured series stays empty rather than plotting a zero.
        assert_eq!(panel.perf_rows.len(), 11);
        let row = |name: &str| {
            panel
                .perf_rows
                .iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("perf row {name}"))
                .value_ms
        };
        assert_eq!(row("Template match"), Some(4.25));
        assert_eq!(row("Total"), Some(9.5));
        assert_eq!(row("ROI crop"), None);

        // The OCR panels carry the live parse rows.
        assert_eq!(panel.ocr_panels.len(), 3);
        assert_eq!(panel.ocr_panels[0].rows.len(), 1);
        assert_eq!(panel.ocr_panels[0].rows[0][0], "Affix_AttackSpeed");
        assert_eq!(panel.ocr_panels[0].rows[0][2], "+18.0% Attack Speed");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The periodic scan re-reads `debug-output/` only when the
    /// directory's mtime moved (or is too recent to vouch for the cache), and
    /// every result equals a fresh full scan.
    #[test]
    fn debug_output_watcher_rescans_only_when_the_directory_changed() {
        let root = debug_test_root("output-watcher");
        let dir = root.join("debug-output");
        let age_dir = |seconds: u64| {
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds);
            std::fs::File::open(&dir)
                .and_then(|handle| handle.set_modified(when))
                .unwrap();
        };
        let mut watcher = DebugOutputWatcher::default();

        // Missing directory: empty, and stays cached while it is missing.
        assert_eq!(*watcher.scan(&dir), scan_debug_output(&dir));
        assert_eq!(watcher.scan(&dir).bundle, None);
        assert_eq!(watcher.reads, 1);

        std::fs::create_dir_all(dir.join("export-100")).unwrap();
        std::fs::write(dir.join("screenshot-50.png"), b"png").unwrap();
        // Fresh mtime: inside the racy window, so it rescans every time.
        assert_eq!(watcher.scan(&dir).bundle_name(), Some("export-100"));
        watcher.scan(&dir);
        assert_eq!(watcher.reads, 3);

        age_dir(60);
        assert_eq!(*watcher.scan(&dir), scan_debug_output(&dir));
        let reads = watcher.reads;
        for _ in 0..3 {
            assert_eq!(*watcher.scan(&dir), scan_debug_output(&dir));
        }
        assert_eq!(
            watcher.reads, reads,
            "an unchanged directory is not re-read"
        );

        // A file written inside an existing bundle leaves the directory
        // itself unchanged, and so does the scan result.
        std::fs::write(dir.join("export-100/timings.json"), "{}").unwrap();
        assert_eq!(*watcher.scan(&dir), scan_debug_output(&dir));
        assert_eq!(watcher.reads, reads);

        // A new bundle changes the directory mtime: re-read.
        std::fs::create_dir_all(dir.join("export-200")).unwrap();
        age_dir(30);
        assert_eq!(watcher.scan(&dir).bundle_name(), Some("export-200"));
        assert_eq!(watcher.reads, reads + 1);
        assert_eq!(*watcher.scan(&dir), scan_debug_output(&dir));
        assert_eq!(watcher.reads, reads + 1);

        // Another directory never reuses the cached scan.
        let other = root.join("debug-output/other");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(watcher.scan(&other).bundle, None);
        assert_eq!(watcher.reads, reads + 2);
    }

    /// A scan taken while the directory stamp was still racy is never
    /// reused: on a coarse-granularity filesystem a later change in the same
    /// tick leaves the stamp unchanged, and the aged stamp must not vouch
    /// for the scan that missed it.
    #[test]
    fn debug_output_watcher_never_reuses_a_scan_taken_under_a_racy_stamp() {
        let root = debug_test_root("output-watcher-racy");
        let dir = root.join("debug-output");
        std::fs::create_dir_all(dir.join("export-100")).unwrap();
        let tick = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
        let set_stamp = || {
            std::fs::File::open(&dir)
                .and_then(|handle| handle.set_modified(tick))
                .unwrap();
        };
        set_stamp();
        let mut watcher = DebugOutputWatcher::default();
        let now = std::time::SystemTime::now();
        assert_eq!(watcher.scan_at(&dir, now).bundle_name(), Some("export-100"));

        // A second change inside the same (coarse) timestamp tick.
        std::fs::create_dir_all(dir.join("export-200")).unwrap();
        set_stamp();

        let later = now + DEBUG_OUTPUT_RACY_WINDOW * 3;
        assert_eq!(
            watcher.scan_at(&dir, later).bundle_name(),
            Some("export-200"),
            "the aged stamp must not vouch for the scan taken while it was racy"
        );
        assert_eq!(*watcher.scan_at(&dir, later), scan_debug_output(&dir));
    }

    /// The per-stage timing table never looks like the live frame.
    /// With a stored run the line names the run it came from, when it was
    /// captured and how old it is; without one it says so and names the live
    /// values as the running loop's own.
    #[test]
    fn stored_run_timings_name_their_source_and_age() {
        let root = debug_test_root("perf-provenance");
        let dir = root.join("debug-output");

        // No bundle at all: the table states the absence instead of posing as
        // the live numbers above it.
        let empty = perf_source_text(&scan_debug_output(&dir));
        assert!(
            empty.starts_with("No stored run yet"),
            "the empty table names its state: {empty}"
        );
        assert!(
            empty.contains("The live values above come from the running loop"),
            "the empty table separates itself from the live values: {empty}"
        );

        // A stored run from two hours ago: named, timed, aged, and explicitly
        // not the live frame.
        let captured = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(7200);
        let bundle = dir.join(format!("export-{captured}"));
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("timings.json"), "{}").unwrap();

        let text = perf_source_text(&scan_debug_output(&dir));
        assert!(
            text.contains(&format!("Stored run export-{captured}/timings.json")),
            "the stored run is named: {text}"
        );
        assert!(
            text.contains("2 h "),
            "the stored run's age is shown: {text}"
        );
        assert!(
            text.contains("not the live frame above"),
            "the stored run is separated from the live values: {text}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The age rendering of the stored-run line, at its boundaries.
    #[test]
    fn stored_run_age_reads_in_seconds_minutes_hours_and_days() {
        let now = std::time::SystemTime::now();
        let ago = |seconds: u64| now - std::time::Duration::from_secs(seconds);

        assert_eq!(age_text(ago(5)), "5 s ago");
        assert_eq!(age_text(ago(59)), "59 s ago");
        assert_eq!(age_text(ago(60)), "1 min ago");
        assert_eq!(age_text(ago(3599)), "59 min ago");
        assert_eq!(age_text(ago(3600)), "1 h 0 min ago");
        assert_eq!(age_text(ago(7200)), "2 h 0 min ago");
        assert_eq!(age_text(ago(86_400)), "1 d ago");
    }

    /// Without any debug capture the panels stay in their empty
    /// state: no artifact links, no stage image, unmeasured timings.
    #[test]
    fn panel_projection_without_a_bundle_stays_empty() {
        let root = debug_test_root("panels-empty");
        let state = fresh_state();
        let panel = debug_panel_state_for_dir(&state, &root);

        assert_eq!(panel.debug_artifact_count, 0);
        assert!(panel.stage_panels.iter().all(|stage| stage.path.is_none()));
        assert_eq!(panel.perf_rows.len(), 11);
        assert!(panel.perf_rows.iter().all(|row| row.value_ms.is_none()));

        std::fs::remove_dir_all(&root).ok();
    }

    /// The artifact link contract: files inside the checkout become
    /// `debug-output/…` links (the only shape `is_debug_output_path`
    /// accepts), files outside it keep their absolute path.
    #[test]
    fn debug_link_path_prefers_workspace_relative_paths() {
        let root = Path::new("/repo");
        let inside = Path::new("/repo/debug-output/export-1/tooltip-crop.png");
        assert_eq!(
            debug_link_path_in(Some(root), inside),
            "debug-output/export-1/tooltip-crop.png"
        );
        let outside = Path::new("/tmp/d4lc/debug-output/export-1/tooltip-crop.png");
        assert_eq!(
            debug_link_path_in(Some(root), outside),
            outside.display().to_string()
        );
        assert_eq!(
            debug_link_path_in(None, inside),
            inside.display().to_string()
        );
    }

    #[test]
    fn detect_timing_ms_reads_bundle_numbers_and_rejects_missing_keys() {
        let timings: serde_json::Value = serde_json::from_str(
            "{\n  \"detect\": {\n    \"match_ms\": 4.25,\n    \"count\": 3,\n    \"x\": \"text\"\n  }\n}\n",
        )
        .unwrap();
        assert_eq!(detect_timing_ms(&timings, "match_ms"), Some(4.25));
        assert_eq!(detect_timing_ms(&timings, "count"), Some(3.0));
        assert_eq!(detect_timing_ms(&timings, "absent_ms"), None);
        assert_eq!(detect_timing_ms(&timings, "x"), None);
    }
}
