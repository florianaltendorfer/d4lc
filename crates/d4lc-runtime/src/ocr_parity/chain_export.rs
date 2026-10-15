//! Debug export of the observable OCR chain of one execution.

use serde_json::json;
use std::path::{Path, PathBuf};

use d4lc_core::{CaptureFrame, Rect};
use d4lc_ocr::{OcrImage, OcrImagePixels};

use super::types::{
    ocr_input_trace, segment_ocr_image, OcrInputTrace, SegmentResult, TooltipOcrExecution,
    TooltipOcrParityResult,
};

// ---------------------------------------------------------------------------
// Observable OCR chain
// ---------------------------------------------------------------------------

/// Files one [`export_tooltip_ocr_chain`] run wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipOcrChainArtifacts {
    /// The final tooltip crop the segments were read from.
    pub tooltip_crop: PathBuf,
    /// Per-segment image exactly as handed to the OCR backend, in the order of
    /// [`TooltipOcrExecution::results`].
    pub segment_inputs: Vec<PathBuf>,
    /// The chain itself: crop trace, segment plans, OCR input traces, raw
    /// text, TSV, normalized lines, name lines with provenance and the parsed
    /// affix rows with their source segment and line.
    pub chain_json: PathBuf,
}

/// Writes the complete observable chain of one tooltip OCR execution:
/// tooltip crop, every segment image exactly as handed to the OCR backend,
/// the raw text, the structured (TSV) output, the normalized lines and the
/// resulting item name with its provenance.
///
/// The segment images are re-derived from the execution's own
/// [`TooltipOcrExecution::ocr_frame`] through the recorded [`OcrInputTrace`]
/// and checked against it: an artifact that does not match the recognized
/// pixels fails instead of being written as evidence.
pub fn export_tooltip_ocr_chain(
    dir: &Path,
    label: &str,
    execution: &TooltipOcrExecution,
    result: &TooltipOcrParityResult,
) -> Result<TooltipOcrChainArtifacts, String> {
    let ocr_frame = &execution.ocr_frame;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;

    let tooltip_crop = dir.join(format!("{label}-tooltip-crop.png"));
    write_bgra_png(&tooltip_crop, ocr_frame)?;

    let mut segment_inputs = Vec::with_capacity(execution.results.len());
    for segment in &execution.results {
        let image = ocr_image_from_trace(ocr_frame, segment.plan.rect, &segment.ocr_input)?;
        let path = dir.join(format!(
            "{label}-segment-{:03}-{}-input.png",
            segment.plan.index,
            segment.plan.kind.as_str()
        ));
        write_ocr_image_png(&path, &image)?;
        segment_inputs.push(path);
    }

    let chain_json = dir.join(format!("{label}-chain.json"));
    let document = json!({
        "schema_version": 1,
        "label": label,
        "preprocess_variant": execution.preprocess_variant.as_str(),
        "preprocess_rule": execution.preprocess_variant.upstream_rule(),
        "tooltip_crop": {
            "rect": execution.crop_trace.final_tooltip_crop_rect,
            "image": tooltip_crop.display().to_string(),
            "size": { "width": ocr_frame.width, "height": ocr_frame.height },
        },
        "crop_source": execution.crop_trace.crop_source,
        "segments": execution.results.iter().map(segment_chain_json).collect::<Vec<_>>(),
        "normalized_lines": result.normalized_lines,
        "name_lines": result.item.name_lines,
        "name_line_sources": result.item.name_line_sources.iter().map(|line| json!({
            "text": line.normalized_text,
            "source_segment_index": line.source_segment_index,
            "source_line_index": line.source_line_index,
            "global_line_index": line.global_line_index,
            "segment_kind": line.segment_kind,
            "rect": line.rect,
        })).collect::<Vec<_>>(),
        "display_name": result.item.display_name,
        "item_type_id": result.item.item_type_id,
        "rarity": result.item.rarity,
        "item_power": result.item.item_power,
        "status": result.item.status,
        "affix_rows": result.item.affixes.iter().map(|affix| json!({
            "row_id": affix.row_id,
            "name": affix.name,
            "value_text": affix.value_text,
            "range_text": affix.range_text,
            "text_value": affix.text_value,
            "affix_id": affix.affix_id,
            "source_segment_index": affix.source.source_segment_index,
            "source_line_index": affix.source.source_line_index,
            "global_line_index": affix.source.global_line_index,
        })).collect::<Vec<_>>(),
        "read_top": result.read_top,
        "segment_count": result.segment_count,
        "line_count": result.line_count,
    });
    let mut text = serde_json::to_string_pretty(&document).unwrap_or_default();
    text.push('\n');
    std::fs::write(&chain_json, text)
        .map_err(|error| format!("failed to write {}: {error}", chain_json.display()))?;

    Ok(TooltipOcrChainArtifacts {
        tooltip_crop,
        segment_inputs,
        chain_json,
    })
}

/// The segment image of a recorded [`OcrInputTrace`], re-derived from the OCR
/// frame and verified against the trace.
fn ocr_image_from_trace(
    ocr_frame: &CaptureFrame,
    rect: Rect,
    trace: &OcrInputTrace,
) -> Result<OcrImage, String> {
    let image = segment_ocr_image(ocr_frame, rect, trace.variant)?;
    let derived = ocr_input_trace(&image, trace.variant);
    if derived != *trace {
        return Err(format!(
            "the re-derived OCR input of segment {} differs from the recognized image \
             (variant {} payload {} {}x{} digest {} != recorded digest {})",
            rect.x,
            trace.variant.as_str(),
            trace.payload,
            trace.width,
            trace.height,
            derived.digest,
            trace.digest
        ));
    }
    Ok(image)
}

fn write_bgra_png(path: &Path, frame: &CaptureFrame) -> Result<(), String> {
    // The chain artifacts are diagnostic output, so the pixel buffer is
    // converted in place instead of per pixel.
    let mut rgba = frame.pixels_bgra.clone();
    for chunk in rgba.as_chunks_mut::<4>().0 {
        chunk.swap(0, 2);
    }
    image::save_buffer(
        path,
        &rgba,
        frame.width,
        frame.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn write_ocr_image_png(path: &Path, image: &OcrImage) -> Result<(), String> {
    match &image.pixels {
        OcrImagePixels::Bgra(frame) => write_bgra_png(path, frame),
        OcrImagePixels::Gray(gray) => image::save_buffer(
            path,
            &gray.pixels,
            gray.width,
            gray.height,
            image::ExtendedColorType::L8,
        )
        .map_err(|error| format!("failed to write {}: {error}", path.display())),
    }
}

fn segment_chain_json(segment: &SegmentResult) -> serde_json::Value {
    json!({
        "index": segment.plan.index,
        "segment_kind": segment.plan.kind.as_str(),
        "rect": segment.plan.rect,
        "ocr_input": {
            "variant": segment.ocr_input.variant.as_str(),
            "payload": segment.ocr_input.payload,
            "size": { "width": segment.ocr_input.width, "height": segment.ocr_input.height },
            "digest": segment.ocr_input.digest,
        },
        "raw_text": segment.text.raw_text,
        "tsv": segment.tsv,
        "converted_text": segment.converted_text,
        "bounding_box_source": segment.text.bounding_box_source.as_str(),
        "lines": segment.text.lines.iter().map(|line| json!({
            "text": line.text,
            "rect": line.rect,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests;
