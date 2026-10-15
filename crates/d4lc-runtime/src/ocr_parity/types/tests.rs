use super::*;
use crate::ocr_parity::test_support::{put_black_pixel, white_frame};

/// The upstream variant must hand Tesseract exactly the grayscale,
/// binary-inverted crop the settings thresholds describe; the pre-fix
/// variant hands over the BGRA crop. Same plan, different pixels.
#[test]
fn preprocess_variant_selects_the_pixels_handed_to_the_ocr_backend() {
    let mut frame = white_frame(8, 2);
    put_black_pixel(&mut frame, 3, 1);
    let rect = Rect {
        x: 1,
        y: 0,
        width: 6,
        height: 2,
    };

    let upstream = segment_ocr_image(
        &frame,
        rect,
        TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv {
            threshold_min: 70,
            threshold_max: 255,
        },
    )
    .unwrap_or_else(|error| panic!("upstream preprocessing failed: {error}"));
    let cropped =
        crop_frame(&frame, rect).unwrap_or_else(|error| panic!("synthetic crop failed: {error}"));
    let expected = threshold_binary_inv(&to_grayscale(&cropped), 70, 255);
    match &upstream.pixels {
        OcrImagePixels::Gray(gray) => assert_eq!(*gray, expected),
        other => panic!("the upstream variant must produce a gray payload, got {other:?}"),
    }

    let raw = segment_ocr_image(&frame, rect, TooltipOcrPreprocessVariant::RawBgra)
        .unwrap_or_else(|error| panic!("raw preprocessing failed: {error}"));
    match &raw.pixels {
        OcrImagePixels::Bgra(cropped_image) => {
            assert_eq!((cropped_image.width, cropped_image.height), (6, 2));
            assert_eq!(cropped_image.pixels_bgra, cropped.pixels_bgra);
        }
        other => panic!("the raw variant must keep the BGRA crop, got {other:?}"),
    }

    assert_ne!(
        ocr_input_trace(
            &upstream,
            TooltipOcrPreprocessVariant::UpstreamGrayBinaryInv {
                threshold_min: 70,
                threshold_max: 255,
            }
        )
        .digest,
        ocr_input_trace(&raw, TooltipOcrPreprocessVariant::RawBgra).digest,
        "the two variants must hand different pixels to the backend"
    );
}
