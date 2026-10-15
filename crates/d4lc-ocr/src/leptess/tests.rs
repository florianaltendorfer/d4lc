use super::*;

#[cfg(feature = "leptess-backend")]
#[test]
fn bgra_rows_become_rgba_without_stride_padding() {
    let frame = CaptureFrame {
        width: 2,
        height: 2,
        stride: 12,
        pixels_bgra: vec![
            1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, //
            9, 10, 11, 12, 13, 14, 15, 16, 99, 99, 99, 99,
        ],
    };
    let mut rgba = vec![7; 3];
    assert!(bgra_rows_to_rgba(&frame, &mut rgba).is_ok());
    assert_eq!(
        rgba,
        [3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]
    );
    let short = CaptureFrame {
        pixels_bgra: vec![0; 19],
        ..frame
    };
    assert!(matches!(
        bgra_rows_to_rgba(&short, &mut rgba),
        Err(OcrError::InvalidImage(_))
    ));
}

#[test]
fn tesseract_debug_sink_uses_platform_null_device() {
    #[cfg(windows)]
    assert_eq!(tesseract_debug_file_sink(), "NUL");
    #[cfg(not(windows))]
    assert_eq!(tesseract_debug_file_sink(), "/dev/null");
}
