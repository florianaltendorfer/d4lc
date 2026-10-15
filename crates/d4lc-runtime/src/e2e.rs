use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use d4lc_capture::{CaptureBackend, SyntheticCaptureFrameSource};
use d4lc_core::{CaptureFrame, InputMode, Point, Rect};
use d4lc_detect::prepared::{PreparedDetector, ThresholdSettings};
use d4lc_parse::{
    ItemTooltipAffixRowGeometry, ItemTooltipNonAffixRow, ItemTooltipParseStatus,
    TooltipOcrSegmentKind,
};

use crate::pipeline::{mock_ocr_snapshot, mock_parse_result, process_one_frame, FrameStatus};
use crate::prepared::{FrameSource, ScriptStep, ScriptedFrameSource};
use crate::reload::{apply_pending_reload, ReloadOutcome, RuntimeReloadSignal};
use crate::{
    render_frame_chain, DiagnosticsClock, DiagnosticsContext, DiagnosticsHandle,
    DiagnosticsOutcome, DiagnosticsVerbosity, DiscardReason, FrameTimeline, OcrJob, OcrWorker,
    PreparedRuntime, Revision, RuntimeSession, SyntheticCursorSource, TooltipCaptureInfo,
    TooltipImage,
};
use d4lc_core::coords::{FramePlacement, FrameRect, TooltipPlacement};

fn white_frame(width: u32, height: u32) -> CaptureFrame {
    let pixels = (0..width * height)
        .flat_map(|_| [255, 255, 255, 255])
        .collect::<Vec<_>>();
    CaptureFrame {
        width,
        height,
        stride: usize::try_from(width).unwrap_or(0) * 4,
        pixels_bgra: pixels,
    }
}

fn put_black_pixel(frame: &mut CaptureFrame, x: usize, y: usize) {
    let width = usize::try_from(frame.width).unwrap_or(0);
    let index = (y * width + x) * 4;
    frame.pixels_bgra[index..index + 4].copy_from_slice(&[0, 0, 0, 255]);
}

fn synthetic_tooltip_frame() -> CaptureFrame {
    let mut frame = white_frame(4, 4);
    put_black_pixel(&mut frame, 1, 3);
    frame
}

/// The synthetic tooltip frame with one gray pixel inside the tooltip crop
/// that does not win the template match: same geometry, different content.
fn changed_crop_frame() -> CaptureFrame {
    let mut frame = synthetic_tooltip_frame();
    // Gray pixel at (x=2, y=0) — first row, third column.
    let index = 2 * 4;
    frame.pixels_bgra[index..index + 4].copy_from_slice(&[128, 128, 128, 255]);
    frame
}

fn synthetic_detector() -> PreparedDetector {
    PreparedDetector {
        preset: "synthetic-e2e".to_string(),
        mode: InputMode::Controller,
        scan_height_percent: 100.0,
        thresholds: ThresholdSettings {
            min: 0,
            max: 255,
            similarity_tooltip: 0.01,
        },
        tooltip_width: 2,
        templates: vec![(
            "synthetic-template.png".to_string(),
            d4lc_core::GrayImage {
                width: 1,
                height: 1,
                pixels: vec![0],
            },
        )],
        scan: d4lc_detect::prepared::DetectScan::default(),
    }
}

fn spawn_parsed_ocr_worker() -> OcrWorker {
    OcrWorker::spawn(|_job| {
        let mut snapshot = mock_ocr_snapshot();
        snapshot.ocr_ms = 2;
        snapshot.capture_ms = 1;
        snapshot.conversion_ms = 1;
        snapshot.segment_count = 1;
        snapshot.line_count = 1;
        snapshot.raw_text = "Synthetic Helm".to_string();
        snapshot.normalized_lines = vec!["Synthetic Helm".to_string()];
        snapshot.text_preview = "Synthetic Helm".to_string();

        let mut parsed = mock_parse_result(ItemTooltipParseStatus::Parsed);
        parsed.display_name = Some("Synthetic Helm".to_string());
        parsed.affix_row_geometry.push(ItemTooltipAffixRowGeometry {
            row_id: d4lc_parse::AffixRowId(0),
            source_segment_index: 0,
            source_line_index: 0,
            global_line_index: 1,
            marker_ordinal: None,
            text_rect: d4lc_core::coords::WindowRect(Rect {
                x: 105,
                y: 46,
                width: 100,
                height: 12,
            }),
            affix_area: d4lc_core::coords::WindowRect(Rect {
                x: 105,
                y: 46,
                width: 100,
                height: 12,
            }),
            marker_anchor: None,
        });
        parsed.non_affix_rows.push(ItemTooltipNonAffixRow {
            kind: TooltipOcrSegmentKind::Rune,
            id: Some("Item_Rune_Condition_OnMove".to_string()),
            label: None,
            source: d4lc_parse::ItemTooltipSourceLine {
                source_segment_index: 1,
                source_line_index: 0,
                global_line_index: 2,
                segment_kind: TooltipOcrSegmentKind::Rune,
                raw_text: "Travel meters.".to_string(),
                converted_text: None,
                normalized_text: "travel meters.".to_string(),
                rect: None,
                marker_name: None,
            },
            rect: Some(Rect {
                x: 110,
                y: 500,
                width: 220,
                height: 18,
            }),
            marker_anchor: None,
        });
        snapshot.item = Some(parsed);

        Ok(snapshot)
    })
}

fn synthetic_runtime_with_worker(
    worker: OcrWorker,
) -> (
    PreparedRuntime,
    RuntimeSession,
    d4lc_capture::CaptureReference,
) {
    let synthetic_source = SyntheticCaptureFrameSource::manual_region(synthetic_tooltip_frame())
        .unwrap_or_else(|error| panic!("synthetic capture setup failed: {error}"));
    let capture_reference = synthetic_source.reference().clone();
    let runtime = PreparedRuntime::new(
        CaptureBackend::X11Native,
        capture_reference.clone(),
        synthetic_detector(),
    )
    .with_synthetic_capture_source(synthetic_source);
    let session = RuntimeSession::new(Some(worker), Duration::from_millis(250));
    (runtime, session, capture_reference)
}

/// Structural gate: the frame hotpath must only pay submit/poll
/// overhead for OCR — a worker whose OCR takes far longer than a frame budget
/// must not extend `hotpath_ms` beyond that overhead.
#[test]
fn ocr_worker_keeps_slow_ocr_off_the_frame_hotpath() {
    let sleep = Duration::from_millis(120);
    let slow_worker = OcrWorker::spawn(move |_job| {
        thread::sleep(sleep);
        let mut snapshot = mock_ocr_snapshot();
        snapshot.ocr_ms = 120;
        Ok(snapshot)
    });
    let (runtime, mut session, _) = synthetic_runtime_with_worker(slow_worker);

    for frame_index in 0..20u64 {
        // Keep the worker busy so every frame polls and submits against a
        // worker that is executing slow OCR.
        if let Some(worker) = session.ocr_worker_ref() {
            if !worker.is_busy() {
                submit_slow_job(worker, frame_index);
            }
        }
        let result = process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("e2e failed: {error}"));
        assert!(
            result.overlay_frame.hotpath_ms < 60,
            "OCR leaked into the frame hotpath: {} ms",
            result.overlay_frame.hotpath_ms
        );
    }

    // The worker must actually have been busy with slow OCR while the frames
    // ran, otherwise this test proves nothing. Busy time accumulates when a
    // job completes, so wait for the in-flight job before reading stats.
    let mut stats = session
        .ocr_stats()
        .unwrap_or_else(|| panic!("ocr stats missing"));
    for _ in 0..1000 {
        if stats.completed > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(2));
        stats = session
            .ocr_stats()
            .unwrap_or_else(|| panic!("ocr stats missing"));
    }
    assert!(
        stats.submitted >= 1,
        "benchmark must submit at least one OCR job"
    );
    assert!(
        stats.total_busy_ms >= 100,
        "worker never executed the slow OCR job; hotpath check is vacuous"
    );
}

fn submit_slow_job(worker: &OcrWorker, frame_index: u64) {
    worker.submit(synthetic_job(frame_index, 7));
}

/// A job over the synthetic tooltip crop (marker at (1, 3), tooltip width 2).
fn synthetic_job(frame_id: u64, tooltip_signature: u64) -> OcrJob {
    let frame = synthetic_tooltip_frame();
    let crop_rect = Rect {
        x: 1,
        y: 0,
        width: 2,
        height: 3,
    };
    let crop = d4lc_capture::crop_frame(&frame, crop_rect)
        .unwrap_or_else(|error| panic!("synthetic crop failed: {error}"));
    OcrJob {
        tooltip_signature,
        token: crate::AnalysisToken::default(),
        image: TooltipImage::new(
            crop,
            TooltipCaptureInfo {
                frame_id,
                captured_at_micros: 0,
                placement: TooltipPlacement {
                    frame: FramePlacement::default(),
                    crop_in_frame: FrameRect(crop_rect),
                },
            },
        ),
    }
}

fn wait_for_worker_completion(worker: &OcrWorker) {
    for _ in 0..100 {
        if worker.stats().completed > 0 {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out waiting for synthetic OCR worker result");
}

#[test]
fn process_one_frame_with_synthetic_capture_source_propagates_non_stub_overlay_state() {
    let synthetic_source = SyntheticCaptureFrameSource::manual_region(synthetic_tooltip_frame())
        .unwrap_or_else(|error| panic!("synthetic capture setup failed: {error}"));
    let capture_reference = synthetic_source.reference().clone();

    let runtime = PreparedRuntime::new(
        CaptureBackend::X11Native,
        capture_reference.clone(),
        synthetic_detector(),
    )
    .with_synthetic_capture_source(synthetic_source);

    let worker = spawn_parsed_ocr_worker();
    let mut session = RuntimeSession::new(Some(worker), Duration::from_millis(250));

    // Frame 0 observes the tooltip, frame 1 is stable and plans the job;
    // frame 2 adopts its result. Only a result the coordinator planned is
    // ever adopted.
    for _ in 0..2 {
        process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("e2e failed: {error}"));
    }
    settle_worker(&session);
    let result = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("e2e failed: {error}"));

    assert_eq!(result.frame_status, FrameStatus::Parsed);
    assert!(result.overlay_frame.tooltip_found);
    assert!(result.overlay_frame.hotpath_ms >= 1);
    assert_eq!(result.overlay_frame.sequence, 2);
    assert_eq!(result.analysis.item, result.current_item);
    assert_eq!(
        session
            .latest_ocr_result()
            .map(|snapshot| snapshot.sequence),
        Some(0)
    );
    assert_eq!(
        result
            .current_item
            .as_ref()
            .and_then(|item| item.item_name.as_deref()),
        Some("Synthetic Helm")
    );
    assert_eq!(
        result
            .current_item
            .as_ref()
            .map(|item| item.parse_status.as_str()),
        Some("Parsed")
    );

    // Real geometry transport: the detected tooltip rect (marker matched at
    // (1, 3), tooltip width 2 → upstream crop rect, window-relative) and the
    // per-affix row geometry carried by the OCR parse result must both reach
    // the frame result instead of being dropped.
    assert_eq!(
        result.tooltip_window_relative,
        Some(Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 3,
        })
    );
    let expected_rows = [ItemTooltipAffixRowGeometry {
        row_id: d4lc_parse::AffixRowId(0),
        source_segment_index: 0,
        source_line_index: 0,
        global_line_index: 1,
        marker_ordinal: None,
        text_rect: d4lc_core::coords::WindowRect(Rect {
            x: 105,
            y: 46,
            width: 100,
            height: 12,
        }),
        affix_area: d4lc_core::coords::WindowRect(Rect {
            x: 105,
            y: 46,
            width: 100,
            height: 12,
        }),
        marker_anchor: None,
    }];
    assert_eq!(
        result
            .current_item
            .as_ref()
            .map(|item| item.affix_row_geometry.as_slice()),
        Some(expected_rows.as_slice())
    );
    // The classified non-affix rows transport through the same snapshot —
    // the synthetic source fabricates the parse result (it cannot produce
    // rune segments), so this proves the snapshot/event transport,
    // not the OCR parity composition.
    let expected_non_affix_rows = [(
        d4lc_parse::TooltipOcrSegmentKind::Rune,
        Some("Item_Rune_Condition_OnMove"),
        Some(Rect {
            x: 110,
            y: 500,
            width: 220,
            height: 18,
        }),
    )];
    let non_affix_rows = result
        .current_item
        .as_ref()
        .map(|item| item.non_affix_rows.as_slice())
        .unwrap_or_default();
    assert_eq!(non_affix_rows.len(), expected_non_affix_rows.len());
    for (row, (kind, id, rect)) in non_affix_rows.iter().zip(expected_non_affix_rows.iter()) {
        assert_eq!(row.kind, *kind);
        assert_eq!(row.id.as_deref(), *id);
        assert_eq!(row.rect, *rect);
    }
}

/// Geometry absence contract: with no tooltip detected in the frame, the
/// result must carry no tooltip rect and no parsed item, so overlay
/// consumers never anchor markers to fabricated geometry.
#[test]
fn process_one_frame_without_tooltip_detection_reports_no_geometry() {
    let synthetic_source = SyntheticCaptureFrameSource::manual_region(white_frame(4, 4))
        .unwrap_or_else(|error| panic!("synthetic capture setup failed: {error}"));
    let capture_reference = synthetic_source.reference().clone();
    let runtime = PreparedRuntime::new(
        CaptureBackend::X11Native,
        capture_reference,
        synthetic_detector(),
    )
    .with_synthetic_capture_source(synthetic_source);
    let mut session = RuntimeSession::new(None, Duration::from_millis(250));

    let result = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("e2e failed: {error}"));

    assert_eq!(result.frame_status, FrameStatus::NoTooltip);
    assert!(!result.overlay_frame.tooltip_found);
    assert_eq!(result.tooltip_window_relative, None);
    assert!(result.current_item.is_none());
}

/// Cursor-source contract: the pipeline polls the prepared cursor source
/// once per frame and forwards the window-relative position into
/// `OverlayFrame` — the input of the overlay menu hover/lock state
/// machine. `None` appears only when the source genuinely has no value
/// for that frame, never as a hardcoded default.
#[test]
fn process_one_frame_polls_the_cursor_source_per_frame_into_the_overlay_frame() {
    let scripted: Arc<Mutex<VecDeque<Option<Point>>>> = Arc::new(Mutex::new(VecDeque::from(vec![
        Some(Point { x: 940, y: 40 }),
        Some(Point { x: 12, y: 34 }),
        None,
    ])));
    let cursor_source = SyntheticCursorSource::new(move || {
        scripted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .flatten()
    });
    let synthetic_source = SyntheticCaptureFrameSource::manual_region(white_frame(4, 4))
        .unwrap_or_else(|error| panic!("synthetic capture setup failed: {error}"));
    let capture_reference = synthetic_source.reference().clone();
    let runtime = PreparedRuntime::new(
        CaptureBackend::X11Native,
        capture_reference,
        synthetic_detector(),
    )
    .with_synthetic_capture_source(synthetic_source)
    .with_synthetic_cursor_source(cursor_source);
    let mut session = RuntimeSession::new(None, Duration::from_millis(250));

    let first = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("first frame failed: {error}"));
    assert_eq!(
        first.overlay_frame.cursor_window_relative,
        Some(Point { x: 940, y: 40 })
    );

    // The cursor is polled per frame, not captured once: the second
    // frame observes the moved pointer.
    let second = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("second frame failed: {error}"));
    assert_eq!(
        second.overlay_frame.cursor_window_relative,
        Some(Point { x: 12, y: 34 })
    );

    // A frame where the source has no value carries None — the overlay
    // hover state machine treats it as "pointer not observed".
    let third = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("third frame failed: {error}"));
    assert_eq!(third.overlay_frame.cursor_window_relative, None);
}

/// The job carries the pixels its own detection ran on. The worker is
/// held while later frames capture different content; the job it finally
/// runs still holds the crop of the frame that was scheduled, and names that
/// frame — no re-capture can swap item B under item A's observation.
#[test]
fn ocr_job_pixels_stay_those_of_the_scheduled_frame_across_later_captures() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let (job_tx, job_rx) = std::sync::mpsc::channel::<OcrJob>();
    let release = Arc::new(AtomicBool::new(false));
    let worker_release = Arc::clone(&release);
    let worker = OcrWorker::spawn(move |job| {
        while !worker_release.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        let _ = job_tx.send(job.clone());
        Ok(mock_ocr_snapshot())
    });
    let (mut runtime, mut session, _) = synthetic_runtime_with_worker(worker);
    let scheduled_frame = changed_crop_frame();

    // Frame 0 observes the tooltip; frame 1 (changed crop, same geometry)
    // is stable and publishes the job.
    process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("first frame failed: {error}"));
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(scheduled_frame.clone())
            .unwrap_or_else(|error| panic!("second synthetic source setup failed: {error}")),
    );
    let scheduled = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("second frame failed: {error}"));
    assert_eq!(
        session.ocr_stats().map(|stats| stats.submitted),
        Some(1),
        "the stable second frame publishes exactly one job"
    );

    // Later captures show different tooltip content at the same place while
    // the job is still waiting on the worker.
    let mut later = synthetic_tooltip_frame();
    // Gray pixel at (x=2, y=1), inside the crop, never the template match.
    later.pixels_bgra[24..28].copy_from_slice(&[200, 200, 200, 255]);
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(later.clone())
            .unwrap_or_else(|error| panic!("later synthetic source setup failed: {error}")),
    );
    for _ in 0..3 {
        process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("later frame failed: {error}"));
    }
    release.store(true, Ordering::SeqCst);

    let job = job_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|error| panic!("no OCR job ran: {error}"));
    let crop_rect = scheduled
        .tooltip_window_relative
        .unwrap_or_else(|| panic!("the scheduled frame detected a tooltip"));
    let expected = d4lc_capture::crop_frame(&scheduled_frame, crop_rect)
        .unwrap_or_else(|error| panic!("expected crop failed: {error}"));
    let later_crop = d4lc_capture::crop_frame(&later, crop_rect)
        .unwrap_or_else(|error| panic!("later crop failed: {error}"));
    assert_ne!(expected.pixels_bgra, later_crop.pixels_bgra);

    assert_eq!(job.image.pixels(), &expected);
    assert_eq!(job.frame_id(), scheduled.overlay_frame.sequence);
    assert_eq!(
        job.image.capture().placement.crop_in_frame,
        FrameRect(crop_rect)
    );
    assert_eq!(
        job.content_fingerprint(),
        crate::hash_capture_rows(&expected, 0),
        "the fingerprint names the carried pixels"
    );
}

/// A worker that records every job it runs (frame id, content fingerprint)
/// and completes it immediately.
fn spawn_recording_worker() -> (OcrWorker, std::sync::mpsc::Receiver<(u64, u64)>) {
    let (tx, rx) = std::sync::mpsc::channel::<(u64, u64)>();
    let worker = OcrWorker::spawn(move |job| {
        let _ = tx.send((job.frame_id(), job.content_fingerprint()));
        Ok(mock_ocr_snapshot())
    });
    (worker, rx)
}

/// Run frames until the worker has finished everything it was given, so the
/// next frame adopts the result.
fn settle_worker(session: &RuntimeSession) {
    if let Some(worker) = session.ocr_worker_ref() {
        for _ in 0..1000 {
            if !worker.is_busy() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("synthetic OCR worker did not go idle");
    }
}

/// An unchanged tooltip `A, A, A, …` is recognized exactly once after
/// the stability condition — no pixel has to change for OCR to start.
#[test]
fn identical_crops_start_ocr_exactly_once() {
    let (worker, jobs) = spawn_recording_worker();
    let (runtime, _, _) = synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
    let mut session = RuntimeSession::new(Some(worker), Duration::ZERO);

    for _ in 0..6 {
        process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("frame failed: {error}"));
        settle_worker(&session);
    }

    let ran: Vec<(u64, u64)> = jobs.try_iter().collect();
    assert_eq!(ran.len(), 1, "exactly one OCR job for A, A, A: {ran:?}");
    assert_eq!(ran[0].0, 1, "submitted on the second, stable observation");
}

/// Item B shown at exactly the geometry of the already recognized
/// item A is new content and is recognized, from B's own pixels.
#[test]
fn new_content_at_the_same_geometry_is_recognized_again() {
    let (worker, jobs) = spawn_recording_worker();
    let (mut runtime, _, _) = synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
    let mut session = RuntimeSession::new(Some(worker), Duration::ZERO);

    for _ in 0..3 {
        process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("item A frame failed: {error}"));
        settle_worker(&session);
    }

    let item_b = changed_crop_frame();
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(item_b.clone())
            .unwrap_or_else(|error| panic!("item B source setup failed: {error}")),
    );
    let mut geometry = Vec::new();
    for _ in 0..3 {
        let result = process_one_frame(&mut session, &runtime)
            .unwrap_or_else(|error| panic!("item B frame failed: {error}"));
        geometry.push(result.tooltip_window_relative);
        settle_worker(&session);
    }

    let ran: Vec<(u64, u64)> = jobs.try_iter().collect();
    assert_eq!(ran.len(), 2, "A once, then B once: {ran:?}");
    let crop_rect = Rect {
        x: 1,
        y: 0,
        width: 2,
        height: 3,
    };
    assert!(geometry.iter().all(|rect| *rect == Some(crop_rect)));
    let item_a_crop = d4lc_capture::crop_frame(&synthetic_tooltip_frame(), crop_rect)
        .unwrap_or_else(|error| panic!("item A crop failed: {error}"));
    let item_b_crop = d4lc_capture::crop_frame(&item_b, crop_rect)
        .unwrap_or_else(|error| panic!("item B crop failed: {error}"));
    assert_eq!(ran[0].1, crate::hash_capture_rows(&item_a_crop, 0));
    assert_eq!(ran[1].1, crate::hash_capture_rows(&item_b_crop, 0));
    assert_ne!(ran[0].1, ran[1].1);
}

/// Settings hot-reload loop contract: a pending reload request swaps in a
/// freshly prepared pair on the loop thread and the NEXT evaluated frame runs
/// on it, without the loop stopping and without the frame sequence resetting.
#[test]
fn pending_reload_applies_new_runtime_to_the_next_evaluated_frame() {
    let (initial_runtime, mut session, _) =
        synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
    let mut runtime = initial_runtime;
    let reload_signal = RuntimeReloadSignal::new();
    // The analysis session identity must survive the reload (asserted below).
    let session_id = session.session_id();

    // Frame under the initially prepared runtime.
    let first = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("first frame failed: {error}"));
    assert_eq!(first.overlay_frame.sequence, 0);

    // Replacement pair, as the app's re-preparation closure would build it:
    // different detector preset and OCR interval from the changed settings.
    let replacement = {
        let synthetic_source =
            SyntheticCaptureFrameSource::manual_region(synthetic_tooltip_frame())
                .unwrap_or_else(|error| panic!("synthetic capture setup failed: {error}"));
        let capture_reference = synthetic_source.reference().clone();
        let mut detector = synthetic_detector();
        detector.preset = "reloaded-preset".to_string();
        let new_runtime =
            PreparedRuntime::new(CaptureBackend::X11Native, capture_reference, detector)
                .with_synthetic_capture_source(synthetic_source);
        let new_session =
            RuntimeSession::new(Some(spawn_parsed_ocr_worker()), Duration::from_millis(999));
        (new_runtime, new_session)
    };

    reload_signal.request();
    let outcome = apply_pending_reload(&reload_signal, &mut runtime, &mut session, || {
        Ok(replacement)
    });
    assert_eq!(outcome, ReloadOutcome::Applied);

    // The next evaluated frame runs on the swapped session: the reloaded
    // preparation values are live and the frame sequence continued instead of
    // resetting (the swap carries the old session's sequence forward).
    let second = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("frame after reload failed: {error}"));
    assert_eq!(session.ocr_interval, Duration::from_millis(999));
    assert_eq!(
        second.overlay_frame.sequence,
        first.overlay_frame.sequence + 1
    );

    // The diagnostics session survives the reload: the swapped-in worker and
    // session emit into the same analysis record instead of starting a second
    // one (a reload is a configuration change inside one session).
    assert_eq!(session.session_id(), session_id);
}

/// Diagnostics chain contract: the events of one processed frame
/// reconstruct a coherent chain — capture → detect → OCR submit → OCR start →
/// OCR finish → parse — with monotonically ordered time points, and a tooltip
/// loss is recorded as the `TooltipLost` discard on the frame it happened.
#[test]
fn diagnostics_frame_chain_is_coherent_for_one_processed_frame() {
    // The OCR signature (marker + template rect) stays identical across the
    // first two frames, so the scheduler submits on the second observation;
    // the crop change on that frame additionally shows up as a new
    // observation generation.

    let handle = DiagnosticsHandle::new(256, DiagnosticsVerbosity::Summary);
    let diagnostics = DiagnosticsContext::new(
        41,
        DiagnosticsClock::new(),
        handle.clone(),
        Revision::Applied(1),
        Revision::Unknown,
    );
    let (mut runtime, mut session, _) = synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
    session.attach_diagnostics(diagnostics.clone());
    assert_eq!(session.session_id(), 41);

    // Frame 0: the tooltip appears. The observation is below the scheduler's
    // stability requirement, so no job is published yet.
    let first = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("first frame failed: {error}"));
    assert!(first.overlay_frame.tooltip_found);

    // Frame 1: the crop changed, the observation is stable — the job is
    // published for this frame.
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(changed_crop_frame())
            .unwrap_or_else(|error| panic!("second synthetic source setup failed: {error}")),
    );
    let second = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("second frame failed: {error}"));
    assert!(second.overlay_frame.tooltip_found);
    let source_frame = second.overlay_frame.sequence;

    // The worker records its own start/finish for that job.
    if let Some(worker) = session.ocr_worker_ref() {
        wait_for_worker_completion(worker);
    }

    // Frame 2: the completed result is adopted and parsed.
    let third = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("third frame failed: {error}"));
    assert_eq!(third.overlay_frame.sequence, source_frame + 1);

    let events = handle.drain();
    let chain = render_frame_chain(&events, 41, source_frame)
        .unwrap_or_else(|| panic!("the submitted frame must have a chain"));
    assert!(
        chain.contains(
            "chain=capture:ok -> detect:ok -> ocr_submit:ok -> ocr_start:ok -> \
             ocr_finish:ok -> parse:ok"
        ),
        "unexpected chain: {chain}"
    );

    let timeline = FrameTimeline::from_events(&events, 41, source_frame)
        .unwrap_or_else(|| panic!("the submitted frame must have a timeline"));
    let captured = timeline.first_captured_at.unwrap_or(u64::MAX);
    let detected = timeline.detected_at.unwrap_or(u64::MAX);
    let queued = timeline.queued_at.unwrap_or(u64::MAX);
    let ocr_started = timeline.ocr_started_at.unwrap_or(u64::MAX);
    let ocr_finished = timeline.ocr_finished_at.unwrap_or(u64::MAX);
    assert!(
        captured <= detected && detected <= queued,
        "capture/detect/submit must be monotonic: {captured} {detected} {queued}"
    );
    assert!(
        queued <= ocr_started && ocr_started <= ocr_finished,
        "submit/start/finish must be monotonic: {queued} {ocr_started} {ocr_finished}"
    );
    // No draw was reported for this frame: the E.2 point stays absent instead
    // of being fabricated.
    assert_eq!(timeline.draw_completed_at, None);

    // Frame ids are monotonic within the session.
    let frame_ids: Vec<u64> = events.iter().map(|event| event.frame_id).collect();
    for pair in frame_ids.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "frame ids must be monotonic within a session: {pair:?}"
        );
    }

    // The tooltip disappears: the next frame records the loss transition with
    // its concrete reason, and the following absent frame records nothing.
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(white_frame(4, 4))
            .unwrap_or_else(|error| panic!("no-tooltip source setup failed: {error}")),
    );
    let lost = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("tooltip-loss frame failed: {error}"));
    assert!(!lost.overlay_frame.tooltip_found);
    let loss_events = handle.drain();
    assert_eq!(
        loss_events.len(),
        1,
        "one loss event, not one per absent frame"
    );
    assert_eq!(
        loss_events[0].outcome,
        DiagnosticsOutcome::discarded(DiscardReason::TooltipLost)
    );
    let loss_chain = render_frame_chain(&loss_events, 41, lost.overlay_frame.sequence)
        .unwrap_or_else(|| panic!("the loss frame must have a chain"));
    assert!(
        loss_chain.contains("chain=detect:discarded(TooltipLost)"),
        "unexpected loss chain: {loss_chain}"
    );

    let still_absent = process_one_frame(&mut session, &runtime)
        .unwrap_or_else(|error| panic!("absent frame failed: {error}"));
    assert!(!still_absent.overlay_frame.tooltip_found);
    assert!(
        handle.drain().is_empty(),
        "an absent frame without a transition stays silent"
    );
}

/// Instrumentation overhead: measure `process_one_frame`
/// with an enabled diagnostics session against the same loop with diagnostics
/// disabled, and report the per-frame delta.
///
/// This is timing evidence from this machine, not a guarantee: the assertion is
/// a coarse regression guard (a per-frame diagnostics cost in the millisecond
/// range would mean the emission path grew work proportional to the buffers).
#[test]
fn diagnostics_overhead_on_the_frame_path_is_measured() {
    fn measure(iterations: u64, enable_diagnostics: bool) -> Duration {
        let (runtime, mut session, _) = synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
        if enable_diagnostics {
            let handle = DiagnosticsHandle::new(4096, DiagnosticsVerbosity::Summary);
            session.attach_diagnostics(DiagnosticsContext::new(
                123,
                DiagnosticsClock::new(),
                handle,
                Revision::Applied(1),
                Revision::Unknown,
            ));
        }
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = process_one_frame(&mut session, &runtime)
                .unwrap_or_else(|error| panic!("frame failed: {error}"));
        }
        started.elapsed()
    }

    let iterations = 300;
    // Warm up both paths once so the measurement is not dominated by the first
    // detection pass (template preparation, allocator growth).
    let _ = measure(20, false);
    let _ = measure(20, true);
    // Interleaved repetitions, keeping each path's best run: the frame path is
    // dominated by detection work, whose scheduler noise is larger than the
    // instrumentation cost this test looks for.
    let mut best_disabled = Duration::MAX;
    let mut best_enabled = Duration::MAX;
    for _ in 0..3 {
        best_disabled = best_disabled.min(measure(iterations, false));
        best_enabled = best_enabled.min(measure(iterations, true));
    }

    let per_frame_disabled = best_disabled.as_micros() / u128::from(iterations);
    let per_frame_enabled = best_enabled.as_micros() / u128::from(iterations);
    let delta = per_frame_enabled.saturating_sub(per_frame_disabled);
    println!(
        "frame-path instrumentation overhead: disabled={per_frame_disabled} us/frame, \
         enabled={per_frame_enabled} us/frame, delta={delta} us/frame over {iterations} frames \
         (best of 3 runs, synthetic 4x4 tooltip frame, summary verbosity)"
    );

    assert!(
        delta < 5_000,
        "diagnostics must not add milliseconds of work per frame, measured {delta} us/frame"
    );
}

// ---------------------------------------------------------------------------
// Runtime coordinator regressions through `process_one_frame`
// ---------------------------------------------------------------------------

/// A 4x4 frame whose tooltip marker (the black template pixel) sits at
/// `(x, y)`: the upstream crop is `x..x+2` wide and `0..y` high.
fn tooltip_frame_with_marker_at(x: usize, y: usize) -> CaptureFrame {
    let mut frame = white_frame(4, 4);
    put_black_pixel(&mut frame, x, y);
    frame
}

/// Set one gray pixel (never a template match) in `frame`.
fn put_gray_pixel(frame: &mut CaptureFrame, x: usize, y: usize, gray: u8) {
    let width = usize::try_from(frame.width).unwrap_or(0);
    let index = (y * width + x) * 4;
    frame.pixels_bgra[index..index + 4].copy_from_slice(&[gray, gray, gray, 255]);
}

fn show(runtime: &mut PreparedRuntime, frame: CaptureFrame) {
    runtime.frame_source = FrameSource::Synthetic(
        SyntheticCaptureFrameSource::manual_region(frame)
            .unwrap_or_else(|error| panic!("synthetic source setup failed: {error}")),
    );
}

fn frame(session: &mut RuntimeSession, runtime: &PreparedRuntime) -> crate::ProcessedFrameResult {
    process_one_frame(session, runtime).unwrap_or_else(|error| panic!("frame failed: {error}"))
}

/// A worker whose jobs block until `release` is set, reporting each job it
/// starts. Results are parsed items that read the crop from `read_top` down.
struct GatedWorker {
    release: Arc<std::sync::atomic::AtomicBool>,
    started: std::sync::mpsc::Receiver<crate::AnalysisToken>,
}

fn spawn_gated_worker(read_top: Option<u32>) -> (OcrWorker, GatedWorker) {
    spawn_gated_worker_reading(read_top, None)
}

/// [`spawn_gated_worker`] whose results read only `read_extent`.
fn spawn_gated_worker_reading(
    read_top: Option<u32>,
    read_extent: Option<crate::ReadExtent>,
) -> (OcrWorker, GatedWorker) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let release = Arc::new(AtomicBool::new(false));
    let worker_release = Arc::clone(&release);
    let (started_tx, started) = std::sync::mpsc::channel();
    let worker = OcrWorker::spawn(move |job| {
        let _ = started_tx.send(job.token);
        while !worker_release.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        let mut snapshot = mock_ocr_snapshot();
        snapshot.ocr_read_top = read_top;
        snapshot.ocr_read_extent = read_extent;
        let mut parsed = mock_parse_result(ItemTooltipParseStatus::Parsed);
        parsed.display_name = Some(format!("Item of frame {}", job.frame_id()));
        snapshot.item = Some(parsed);
        Ok(snapshot)
    });
    (worker, GatedWorker { release, started })
}

impl GatedWorker {
    fn open(&self) {
        self.release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn wait_started(&self) -> crate::AnalysisToken {
        self.started
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("no OCR job started: {error}"))
    }

    fn started_jobs(&self) -> Vec<crate::AnalysisToken> {
        self.started.try_iter().collect()
    }
}

fn wait_for_result(session: &RuntimeSession, completed_before: u64) {
    let worker = session
        .ocr_worker_ref()
        .unwrap_or_else(|| panic!("session has no worker"));
    for _ in 0..5000 {
        let stats = worker.stats();
        if stats.completed + stats.failed > completed_before {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the OCR worker produced no result");
}

fn diagnostics_session(id: u64) -> (DiagnosticsContext, DiagnosticsHandle) {
    let handle = DiagnosticsHandle::new(1024, DiagnosticsVerbosity::Summary);
    let context = DiagnosticsContext::new(
        id,
        DiagnosticsClock::new(),
        handle.clone(),
        Revision::Applied(1),
        Revision::Applied(1),
    );
    (context, handle)
}

fn parse_discards(events: &[crate::DiagnosticsEvent]) -> Vec<DiscardReason> {
    events
        .iter()
        .filter(|event| event.phase == crate::DiagnosticsPhase::Parse)
        .filter_map(crate::DiagnosticsEvent::discard_reason)
        .collect()
}

/// Run frames of a synthetic runtime with `worker` over item A.
fn coordinator_fixture(worker: OcrWorker) -> (PreparedRuntime, RuntimeSession, DiagnosticsHandle) {
    let (runtime, mut session, _) = synthetic_runtime_with_worker(worker);
    let (context, handle) = diagnostics_session(77);
    session.attach_diagnostics(context);
    session.ocr_interval = Duration::ZERO;
    session.coordinator.set_retry_interval(Duration::ZERO);
    (runtime, session, handle)
}

/// Fingerprint robustness: the crop spans from the screen top to the marker,
/// so the rows above the tooltip body are live game scene. A steadily shown
/// tooltip whose scene rows animate on every frame is recognized once; the
/// rows the OCR read (from `ocr_read_top` down) decide "already processed".
#[test]
fn a_steady_tooltip_under_an_animated_scene_is_recognized_once() {
    let (worker, gate) = spawn_gated_worker(Some(1));
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);

    for step in 0..8u8 {
        // Row 0 of the crop (x=2, y=0) is scene above the read region and
        // changes on every frame.
        let mut animated = synthetic_tooltip_frame();
        put_gray_pixel(&mut animated, 2, 0, 100 + step * 10);
        show(&mut runtime, animated);
        frame(&mut session, &runtime);
        settle_worker(&session);
    }

    let jobs = gate.started_jobs();
    assert_eq!(jobs.len(), 1, "one OCR for a steady tooltip: {jobs:?}");
    assert!(session.coordinator().snapshot().has_item());
}

/// The crop is `TooltipWidth` wide, so a tooltip narrower than that has live
/// scene beside it inside the read rows. A steadily shown tooltip whose
/// neighboring scene animates on every frame is recognized once; only the
/// columns of the recognized text decide "already processed".
#[test]
fn a_steady_tooltip_beside_an_animated_scene_is_recognized_once() {
    let extent = crate::ReadExtent {
        left: 0,
        right: 1,
        bottom: 3,
    };
    let (worker, gate) = spawn_gated_worker_reading(Some(1), Some(extent));
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);

    for step in 0..8u8 {
        // (x=2, y=2) lies in the read rows but right of the read columns.
        let mut animated = synthetic_tooltip_frame();
        put_gray_pixel(&mut animated, 2, 2, 100 + step * 10);
        show(&mut runtime, animated);
        frame(&mut session, &runtime);
        settle_worker(&session);
    }

    let jobs = gate.started_jobs();
    assert_eq!(jobs.len(), 1, "one OCR for a steady tooltip: {jobs:?}");
    assert!(session.coordinator().snapshot().has_item());
}

/// In controller mode the crop reaches down to the key-hint marker, and the
/// hint glyphs between the tooltip text and the marker animate. A steadily
/// shown tooltip whose rows below the recognized text change on every frame
/// is recognized once.
#[test]
fn a_steady_tooltip_above_animated_key_hints_is_recognized_once() {
    let extent = crate::ReadExtent {
        left: 0,
        right: 1,
        bottom: 2,
    };
    let (worker, gate) = spawn_gated_worker_reading(Some(1), Some(extent));
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);

    for step in 0..8u8 {
        // (x=1, y=2) lies in the read columns but below the read bottom.
        let mut animated = synthetic_tooltip_frame();
        put_gray_pixel(&mut animated, 1, 2, 100 + step * 10);
        show(&mut runtime, animated);
        frame(&mut session, &runtime);
        settle_worker(&session);
    }

    let jobs = gate.started_jobs();
    assert_eq!(jobs.len(), 1, "one OCR for a steady tooltip: {jobs:?}");
    assert!(session.coordinator().snapshot().has_item());
}

/// With a known read region: a change inside the read rows at the same
/// geometry is new content and recognized again from its own pixels.
#[test]
fn a_change_inside_the_read_rows_is_new_content() {
    let (worker, gate) = spawn_gated_worker(Some(1));
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);

    for _ in 0..3 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }
    let item_a = session
        .coordinator()
        .snapshot()
        .token
        .unwrap_or_else(|| panic!("item A must be accepted"));

    let mut item_b = synthetic_tooltip_frame();
    put_gray_pixel(&mut item_b, 2, 2, 150);
    show(&mut runtime, item_b);
    for _ in 0..4 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }

    let jobs = gate.started_jobs();
    assert_eq!(jobs.len(), 2, "A once, then B once: {jobs:?}");
    let item_b_token = session
        .coordinator()
        .snapshot()
        .token
        .unwrap_or_else(|| panic!("item B must be accepted"));
    assert!(item_b_token.tooltip_generation > item_a.tooltip_generation);
    assert_ne!(item_b_token.content_fingerprint, item_a.content_fingerprint);
}

/// A is being recognized, the tooltip disappears, then A's late result
/// arrives. It is discarded as `TooltipLost` and never shown.
#[test]
fn a_late_result_after_tooltip_loss_is_never_shown() {
    let (worker, gate) = spawn_gated_worker(None);
    let (mut runtime, mut session, handle) = coordinator_fixture(worker);

    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    gate.wait_started();

    show(&mut runtime, white_frame(4, 4));
    let lost = frame(&mut session, &runtime);
    assert!(lost.analysis.item.is_none());
    assert_eq!(
        lost.analysis.invalidation,
        Some(crate::InvalidationCause::TooltipLost)
    );

    gate.open();
    wait_for_result(&session, 0);
    for _ in 0..3 {
        let result = frame(&mut session, &runtime);
        assert!(
            result.current_item.is_none(),
            "the late result must not show"
        );
        assert!(!result.analysis.has_item());
    }
    assert_eq!(
        parse_discards(&handle.drain()),
        vec![DiscardReason::TooltipLost]
    );
}

/// Focus is lost while OCR runs. The analysis is hidden at once — not
/// when OCR ends — and the result that arrives afterwards is discarded.
#[test]
fn focus_loss_during_ocr_hides_immediately_and_discards_the_result() {
    let (worker, gate) = spawn_gated_worker(None);
    let (runtime, mut session, handle) = coordinator_fixture(worker);

    // An accepted analysis first, so there is something to hide.
    gate.open();
    for _ in 0..3 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }
    assert!(session.coordinator().snapshot().has_item());
    assert_eq!(gate.started_jobs().len(), 1);
    let generation_before = session.coordinator().presentation_generation();

    // A new job is in flight when focus is lost (the worker is gated again).
    gate.release
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let mut item_b = synthetic_tooltip_frame();
    put_gray_pixel(&mut item_b, 2, 1, 150);
    let mut runtime = runtime;
    show(&mut runtime, item_b);
    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    gate.wait_started();
    let in_flight_before = session.ocr_stats().map(|stats| stats.completed);

    let cause = session
        .coordinator_mut()
        .set_game_window(crate::GameWindowState {
            focused: false,
            minimized: false,
            closed: false,
        });
    assert_eq!(cause, Some(crate::InvalidationCause::FocusLost));
    let hidden = session.coordinator().snapshot();
    assert!(!hidden.has_item(), "hidden before OCR finished");
    assert!(hidden.presentation_generation > generation_before);
    assert_eq!(
        session.ocr_stats().map(|stats| stats.completed),
        in_flight_before,
        "OCR had not finished when the analysis was hidden"
    );

    gate.open();
    wait_for_result(&session, in_flight_before.unwrap_or(0));
    for _ in 0..3 {
        let result = frame(&mut session, &runtime);
        assert!(!result.analysis.has_item());
        assert_eq!(
            result.analysis.tooltip_state,
            crate::TooltipState::Invalidated
        );
    }
    assert!(parse_discards(&handle.drain()).contains(&DiscardReason::FocusLost));
    assert!(
        gate.started_jobs().is_empty(),
        "no OCR is started while the game has no focus"
    );
}

/// A stopped session discards its late result, and the result of the
/// old session can never change a new session.
#[test]
fn a_late_result_of_a_stopped_session_does_not_touch_a_new_session() {
    let (worker, gate) = spawn_gated_worker(None);
    let (runtime, mut old_session, _) = coordinator_fixture(worker);
    frame(&mut old_session, &runtime);
    frame(&mut old_session, &runtime);
    gate.wait_started();

    old_session.coordinator_mut().begin_stop();
    assert_eq!(
        old_session.coordinator().lifecycle(),
        crate::RuntimeLifecycle::Stopping
    );

    let (new_worker, new_gate) = spawn_gated_worker(None);
    new_gate.open();
    let (_, mut new_session, _) = coordinator_fixture(new_worker);
    let (context, _) = diagnostics_session(78);
    new_session.attach_diagnostics(context);
    for _ in 0..3 {
        frame(&mut new_session, &runtime);
        settle_worker(&new_session);
    }
    let accepted_before = new_session.coordinator().snapshot();
    assert!(accepted_before.has_item());

    gate.open();
    wait_for_result(&old_session, 0);
    let late = old_session
        .ocr_worker_ref()
        .and_then(OcrWorker::poll_latest_result)
        .unwrap_or_else(|| panic!("the old session's late result"));
    assert_eq!(
        old_session.coordinator_mut().on_result(late.clone()),
        crate::ResultDisposition::Discarded(DiscardReason::SessionStopped)
    );
    assert_eq!(
        new_session.coordinator_mut().on_result(late),
        crate::ResultDisposition::Discarded(DiscardReason::SessionStopped)
    );
    assert_eq!(new_session.coordinator().snapshot(), accepted_before);
}

/// The configuration changes while OCR runs. The analysis of the old
/// configuration is invalidated and its late result is discarded.
#[test]
fn a_configuration_change_during_ocr_discards_the_stale_result() {
    let (worker, gate) = spawn_gated_worker(None);
    let (runtime, mut session, handle) = coordinator_fixture(worker);
    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    let token = gate.wait_started();
    assert_eq!(token.config_revision, Revision::Applied(1));

    // The live loop applies a settings reload as a new configuration revision.
    session
        .diagnostics()
        .set_config_revision(Revision::Applied(2));
    gate.open();
    wait_for_result(&session, 0);
    let after = frame(&mut session, &runtime);
    assert!(!after.analysis.has_item());
    let discards = parse_discards(&handle.drain());
    assert_eq!(discards, vec![DiscardReason::ConfigChanged]);

    // The still visible tooltip is analyzed again under the new configuration.
    for _ in 0..3 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }
    let accepted = session
        .coordinator()
        .snapshot()
        .token
        .unwrap_or_else(|| panic!("re-analyzed under the new configuration"));
    assert_eq!(accepted.config_revision, Revision::Applied(2));
}

/// A settings reload swaps the session: the coordinator moves into the new
/// session, and the late result of the replaced worker cannot be adopted.
#[test]
fn a_reload_carries_the_coordinator_and_invalidates_the_old_configuration() {
    let (worker, gate) = spawn_gated_worker(None);
    let (mut runtime, mut session, _) = coordinator_fixture(worker);
    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    gate.wait_started();
    let generation = session.tooltip_generation();

    let signal = RuntimeReloadSignal::new();
    signal.request();
    let replacement = {
        let (new_runtime, new_session, _) =
            synthetic_runtime_with_worker(spawn_parsed_ocr_worker());
        (new_runtime, new_session)
    };
    let outcome = apply_pending_reload(&signal, &mut runtime, &mut session, || Ok(replacement));
    assert_eq!(outcome, ReloadOutcome::Applied);
    session
        .diagnostics()
        .set_config_revision(Revision::Applied(2));
    gate.open();

    let after = frame(&mut session, &runtime);
    assert_eq!(session.session_id(), 77);
    assert!(session.tooltip_generation() > generation);
    assert!(!after.analysis.has_item());
}

/// A pure rule-set change keeps the accepted analysis: no new OCR, the same
/// token — the consumer re-evaluates the item against the new rules.
#[test]
fn a_rules_change_keeps_the_accepted_analysis_without_new_ocr() {
    let (worker, gate) = spawn_gated_worker(None);
    gate.open();
    let (runtime, mut session, _) = coordinator_fixture(worker);
    for _ in 0..3 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }
    let accepted = session.coordinator().snapshot();
    assert!(accepted.has_item());
    assert_eq!(gate.started_jobs().len(), 1);

    session
        .diagnostics()
        .set_rules_revision(Revision::Applied(9));
    for _ in 0..3 {
        let result = frame(&mut session, &runtime);
        assert_eq!(result.analysis.token, accepted.token);
        assert!(result.analysis.has_item());
    }
    assert!(gate.started_jobs().is_empty(), "no OCR for a rules change");
}

/// At most one running and one waiting job. A steady tooltip never
/// queues behind its own job; a new placement waits (replaceably) behind the
/// running job of an older generation.
#[test]
fn at_most_one_running_and_one_waiting_ocr_job() {
    let (worker, gate) = spawn_gated_worker(None);
    let (mut runtime, mut session, _) = coordinator_fixture(worker);

    // Placement 1: submitted and running.
    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    gate.wait_started();
    for _ in 0..5 {
        frame(&mut session, &runtime);
    }
    // Placement 2 and 3 while job 1 still runs.
    for (x, y) in [(2, 3), (1, 2)] {
        show(&mut runtime, tooltip_frame_with_marker_at(x, y));
        for _ in 0..4 {
            frame(&mut session, &runtime);
        }
    }
    let stats = session
        .ocr_stats()
        .unwrap_or_else(|| panic!("worker stats"));
    assert_eq!(stats.submitted, 3, "one job per placement generation");
    assert_eq!(
        stats.dropped, 1,
        "placement 3 replaced the waiting placement 2"
    );
    assert!(stats.max_queue_depth <= 2);
    assert!(stats.pending && stats.in_progress);

    gate.open();
    for _ in 0..4 {
        settle_worker(&session);
        frame(&mut session, &runtime);
    }
    let snapshot = session.coordinator().snapshot();
    assert!(
        snapshot.has_item(),
        "the waiting job of the shown placement is adopted"
    );
    assert_eq!(
        snapshot.tooltip_window_relative,
        Some(Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 2,
        })
    );
}

/// The screen changes while OCR runs: the result describes pixels that are
/// no longer shown. It is superseded, and the new content is analyzed.
#[test]
fn a_result_whose_pixels_are_no_longer_shown_is_superseded() {
    let (worker, gate) = spawn_gated_worker(None);
    let (mut runtime, mut session, handle) = coordinator_fixture(worker);
    frame(&mut session, &runtime);
    frame(&mut session, &runtime);
    let item_a = gate.wait_started();

    let mut item_b = synthetic_tooltip_frame();
    put_gray_pixel(&mut item_b, 2, 1, 150);
    show(&mut runtime, item_b);
    frame(&mut session, &runtime);
    assert!(
        gate.started_jobs().is_empty(),
        "no second job of the same generation while the first runs"
    );

    gate.open();
    wait_for_result(&session, 0);
    let adopted = frame(&mut session, &runtime);
    assert!(
        !adopted.analysis.has_item(),
        "A's result must not describe B"
    );
    assert_eq!(
        parse_discards(&handle.drain()),
        vec![DiscardReason::Superseded]
    );

    for _ in 0..3 {
        frame(&mut session, &runtime);
        settle_worker(&session);
    }
    let item_b_token = session
        .coordinator()
        .snapshot()
        .token
        .unwrap_or_else(|| panic!("B must be accepted"));
    assert_ne!(item_b_token.content_fingerprint, item_a.content_fingerprint);
}

// ── Repeated frames re-feed the previous observation ─────────

fn play(
    steps: Vec<ScriptStep>,
    worker: OcrWorker,
) -> (Vec<crate::ProcessedFrameResult>, RuntimeSession) {
    let (mut runtime, mut session, _) = coordinator_fixture(worker);
    let count = steps.len();
    runtime.frame_source = FrameSource::Scripted(ScriptedFrameSource::new(steps));
    let results = (0..count).map(|_| frame(&mut session, &runtime)).collect();
    (results, session)
}

/// The coordinator reaches `Submit` after `A, repeat` exactly as after
/// `A, A`: the repeat re-feeds A's observation (same signature, same
/// image), so the stability counter advances as with a re-capture.
#[test]
fn a_repeated_frame_reaches_submit_like_a_recaptured_one() {
    let (recaptured_worker, recaptured_gate) = spawn_gated_worker(None);
    recaptured_gate.open();
    // The sessions own the workers: both stay alive until the jobs ran.
    let (recaptured, _recaptured_session) = play(
        vec![
            ScriptStep::Frame(synthetic_tooltip_frame()),
            ScriptStep::Frame(synthetic_tooltip_frame()),
        ],
        recaptured_worker,
    );
    let (repeated_worker, repeated_gate) = spawn_gated_worker(None);
    repeated_gate.open();
    let (repeated, session) = play(
        vec![
            ScriptStep::Frame(synthetic_tooltip_frame()),
            ScriptStep::NoNewFrame,
        ],
        repeated_worker,
    );

    let recaptured_job = recaptured_gate.wait_started();
    let repeated_job = repeated_gate.wait_started();
    assert_eq!(
        recaptured_job.content_fingerprint, repeated_job.content_fingerprint,
        "the repeat submits the same observation"
    );
    assert_eq!(
        repeated_job.source_frame_id, 0,
        "the job carries the pixels of the frame that delivered them"
    );
    assert!(recaptured_gate.started_jobs().is_empty());
    assert!(repeated_gate.started_jobs().is_empty());

    assert_eq!(repeated[1].frame_status, FrameStatus::RepeatedFrame);
    assert_ne!(recaptured[1].frame_status, FrameStatus::RepeatedFrame);
    assert_eq!(
        repeated[1].tooltip_window_relative,
        recaptured[1].tooltip_window_relative
    );
    assert_eq!(
        repeated[1].overlay_frame.tooltip_found,
        recaptured[1].overlay_frame.tooltip_found
    );
    assert_eq!(repeated[1].stage_us.detect_us, 0, "detection was skipped");
    assert!(
        repeated[0]
            .stage_us
            .detect_scan_threads
            .is_some_and(|threads| threads >= 1),
        "a detected frame names its scan threads"
    );
    assert_eq!(
        repeated[1].stage_us.detect_scan_threads, None,
        "a repeat ran no scan and reports no thread count"
    );
    assert_eq!(session.sequence, 2, "a repeat is still one processed frame");
}

/// An absent frame followed by a repeat still counts as absent: a tooltip
/// that disappeared stays lost, and nothing is submitted.
#[test]
fn an_absent_frame_repeated_still_counts_as_absent() {
    let (worker, gate) = spawn_gated_worker(None);
    gate.open();
    let (results, session) = play(
        vec![
            ScriptStep::Frame(synthetic_tooltip_frame()),
            ScriptStep::Frame(white_frame(4, 4)),
            ScriptStep::NoNewFrame,
            ScriptStep::NoNewFrame,
        ],
        worker,
    );
    assert!(!results[1].overlay_frame.tooltip_found);
    for repeat in &results[2..] {
        assert_eq!(repeat.frame_status, FrameStatus::RepeatedFrame);
        assert!(!repeat.overlay_frame.tooltip_found);
        assert_eq!(repeat.tooltip_window_relative, None);
    }
    assert_eq!(
        session.coordinator().snapshot().tooltip_state,
        crate::TooltipState::NoTooltip
    );
    assert!(
        gate.started_jobs().is_empty(),
        "a single tooltip frame before the loss never became stable"
    );
}

/// An invalidation (focus lost) between repeats still blocks submission:
/// the re-fed observation is suppressed exactly like a re-captured one.
#[test]
fn an_invalidation_between_repeats_blocks_submission() {
    let (worker, gate) = spawn_gated_worker(None);
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);
    runtime.frame_source = FrameSource::Scripted(ScriptedFrameSource::new(vec![
        ScriptStep::Frame(synthetic_tooltip_frame()),
        ScriptStep::NoNewFrame,
        ScriptStep::NoNewFrame,
    ]));
    frame(&mut session, &runtime);
    let cause = session
        .coordinator_mut()
        .set_game_window(crate::GameWindowState {
            focused: false,
            minimized: false,
            closed: false,
        });
    assert_eq!(cause, Some(crate::InvalidationCause::FocusLost));
    for _ in 0..2 {
        let result = frame(&mut session, &runtime);
        assert_eq!(result.frame_status, FrameStatus::RepeatedFrame);
        assert!(!result.analysis.has_item());
    }
    settle_worker(&session);
    assert!(
        gate.started_jobs().is_empty(),
        "no OCR is submitted from a repeat while the game has no focus"
    );
}

/// A session with no previous frame (the first frame after a runtime swap)
/// has nothing to re-feed: it observes nothing and says so.
#[test]
fn a_repeat_without_a_previous_frame_observes_nothing() {
    let (worker, gate) = spawn_gated_worker(None);
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);
    runtime.frame_source = FrameSource::Scripted(ScriptedFrameSource::new(vec![
        ScriptStep::Frame(synthetic_tooltip_frame()),
        ScriptStep::NoNewFrame,
    ]));
    // Consume the delivered frame in another session, as the runtime
    // before a swap would have.
    let (_, mut previous, _) = coordinator_fixture(spawn_parsed_ocr_worker());
    frame(&mut previous, &runtime);

    let result = frame(&mut session, &runtime);
    assert_eq!(result.frame_status, FrameStatus::RepeatedFrame);
    assert!(!result.overlay_frame.tooltip_found);
    assert_eq!(
        session.coordinator().snapshot().tooltip_state,
        crate::TooltipState::NoTooltip
    );
    assert!(gate.started_jobs().is_empty());
}

/// A switched-off overlay captures nothing (upstream's capture handler skips
/// the capture while `IsEnabled` is false), and switching it back on resumes
/// with the very next frame: the scripted frame is still unconsumed.
#[test]
fn a_switched_off_overlay_captures_nothing_and_resumes_at_once() {
    let (worker, gate) = spawn_gated_worker(None);
    gate.open();
    let (mut runtime, mut session, _) = coordinator_fixture(worker);
    runtime.frame_source =
        FrameSource::Scripted(ScriptedFrameSource::new(vec![ScriptStep::Frame(
            synthetic_tooltip_frame(),
        )]));
    assert_eq!(
        session.coordinator_mut().set_overlay_enabled(false),
        Some(crate::InvalidationCause::OverlayDisabled)
    );
    for _ in 0..3 {
        let result = frame(&mut session, &runtime);
        assert_eq!(result.frame_status, FrameStatus::OverlayDisabled);
        assert!(!result.overlay_frame.tooltip_found);
        assert!(result.overlay_frame.rule_descriptors.is_empty());
    }
    settle_worker(&session);
    assert!(gate.started_jobs().is_empty());

    assert_eq!(session.coordinator_mut().set_overlay_enabled(true), None);
    let resumed = frame(&mut session, &runtime);
    assert_ne!(resumed.frame_status, FrameStatus::OverlayDisabled);
    assert!(resumed.overlay_frame.tooltip_found);
}
