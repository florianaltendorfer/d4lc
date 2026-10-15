use super::*;
use crate::test_support::tiny_image;

#[test]
fn ocr_options_default_collects_tsv_for_parity() {
    let options = OcrOptions::default();
    assert!(options.collect_tsv);
    assert_eq!(options.page_seg_mode, OcrPageSegMode::SparseText);
}

#[test]
fn gray_ocr_image_reports_gray_dimensions_and_variant() {
    let image = OcrImage::from_gray(GrayImage {
        width: 7,
        height: 3,
        pixels: vec![255; 21],
    });

    assert_eq!(image.variant_name(), "gray");
    assert_eq!((image.width(), image.height()), (7, 3));
    assert_eq!(tiny_image().variant_name(), "bgra");
}

/// Reading the structured output can fail, and that failure is a
/// failed recognition. It is never reported as a text-only success whose
/// lines carry no geometry.
#[test]
fn structured_output_read_failure_is_a_named_backend_error() {
    let read_error = match String::from_utf8(vec![0xFF_u8, 0xFE]) {
        Ok(_) => panic!("0xFF is not valid UTF-8"),
        Err(error) => error,
    };
    let error = structured_output_error(read_error);

    match error {
        OcrError::Backend(message) => assert!(
            message.contains("failed to read OCR structured output (TSV)"),
            "{message}"
        ),
        other => panic!("expected a backend error, got {other:?}"),
    }
}
