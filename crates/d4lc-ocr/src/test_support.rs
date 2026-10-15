use crate::OcrImage;
use d4lc_core::CaptureFrame;

pub(crate) fn tiny_image() -> OcrImage {
    OcrImage::new(CaptureFrame {
        width: 1,
        height: 1,
        stride: 4,
        pixels_bgra: vec![0, 0, 0, 255],
    })
}
