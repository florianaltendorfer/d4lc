use super::prepare::PreparedLiveRuntime;
use super::LiveArgs;
use crate::roots::Roots;
use d4lc_runtime::OcrWorker;
#[cfg(feature = "leptess-backend")]
use d4lc_runtime::{OcrJob, TooltipTextSegmentSnapshot, TooltipTextSnapshot};

pub(super) fn create_ocr_worker(
    roots: &Roots,
    args: &LiveArgs,
    prepared: &PreparedLiveRuntime,
) -> Result<Option<OcrWorker>, String> {
    if !args.ocr_worker {
        return Ok(None);
    }
    build_ocr_worker(roots, args, prepared).map(Some)
}

#[cfg(feature = "leptess-backend")]
fn build_ocr_worker(
    roots: &Roots,
    args: &LiveArgs,
    _prepared: &PreparedLiveRuntime,
) -> Result<OcrWorker, String> {
    use d4lc_ocr::{OcrBackendKind, OcrEngineRegistry, OcrProfile};
    use d4lc_runtime::{process_tooltip_ocr_parity_prepared, PreparedTooltipOcr};

    let settings = roots.settings().map_err(|e| format!("ocr-worker: {e}"))?;
    let preset = args
        .preset
        .clone()
        .unwrap_or_else(|| settings.selected_system_preset.clone());
    let language = args
        .language
        .clone()
        .unwrap_or_else(|| settings.selected_affix_language.clone());
    let profile = OcrProfile {
        backend: OcrBackendKind::Auto,
        language,
        tessdata_path: None,
    };
    let registry = OcrEngineRegistry::new(move |p| {
        let engine = d4lc_ocr::LeptessOcrEngine::new(p)?;
        Ok(Box::new(engine))
    });
    let ocr_preview_chars = usize::try_from(args.ocr_text_preview_chars.unwrap_or(80))
        .unwrap_or(80)
        .max(1);
    // Prepare session state (settings + template images) once per OCR worker;
    // each job then only pays the per-frame OCR work.
    let ocr_session = PreparedTooltipOcr::prepare_with_game_data_root(
        roots.config_path()?,
        &roots.assets,
        preset,
        profile,
    )
    .map_err(|e| format!("ocr-worker prepare: {e}"))?;

    let worker = OcrWorker::spawn(move |job: OcrJob| {
        // The job carries the crop its frame's detection ran on; the worker
        // recognizes exactly those pixels and never captures again.
        let parity = process_tooltip_ocr_parity_prepared(
            &ocr_session,
            &registry,
            job.image.pixels(),
            ocr_preview_chars,
            // Row geometry comes back in window space, converted once
            // through the crop's capture-time placement.
            &job.image.capture().placement,
        )?;

        Ok(TooltipTextSnapshot {
            sequence: 0, // set by worker
            source_frame_sequence: job.frame_id(),
            tooltip_signature: job.tooltip_signature,
            content_fingerprint: job.content_fingerprint(),
            token: job.token,
            ocr_read_top: parity.read_top,
            ocr_read_extent: parity.read_extent,
            parity_path: parity.parity_path.to_string(),
            ocr_ms: parity.ocr_ms,
            // No capture step in the worker: the crop comes with the job.
            capture_ms: 0,
            conversion_ms: parity.conversion_ms,
            splitter_detection_ms: Some(parity.splitter_detection_ms),
            segment_count: parity.segment_count,
            line_count: parity.line_count,
            raw_text: parity.raw_text,
            normalized_lines: parity.normalized_lines,
            segments_preview: parity
                .segments_preview
                .into_iter()
                .map(|segment| TooltipTextSegmentSnapshot {
                    index: segment.index,
                    segment_kind: segment.segment_kind,
                    line_count: segment.line_count,
                    raw_text_preview: segment.raw_text_preview,
                    converted_text_preview: segment.converted_text_preview,
                })
                .collect(),
            title_preview: parity.title_preview,
            text_preview: parity.text_preview,
            item: Some(parity.item),
            error: None,
            debug: None,
        })
    });
    Ok(worker)
}

#[cfg(not(feature = "leptess-backend"))]
fn build_ocr_worker(
    _roots: &Roots,
    _args: &LiveArgs,
    _prepared: &PreparedLiveRuntime,
) -> Result<OcrWorker, String> {
    Err("--ocr-worker requires the leptess-backend feature; rebuild with --features leptess-backend and install libtesseract-dev/libleptonica-dev".to_string())
}
