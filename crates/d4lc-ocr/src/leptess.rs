#[cfg(feature = "leptess-backend")]
use crate::text::raw_text_to_ocr_text;
#[cfg(feature = "leptess-backend")]
use crate::{
    parse_tesseract_tsv_lines, parse_upper_tooltip_text, structured_output_error,
    upstream_convert_to_text, OcrEngine, OcrError, OcrImage, OcrImagePixels, OcrOptions,
    OcrProfile, OcrSegmentKind, OcrStructuredOutput,
};
#[cfg(feature = "leptess-backend")]
use d4lc_core::{CaptureFrame, OcrText};

/// How [`LeptessOcrEngine`] hands an [`OcrImage`] to Tesseract.
#[cfg(feature = "leptess-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeptessImageHandoff {
    /// The pixel rows go straight to `TessBaseAPI::SetImage` (the live path).
    RawPixels,
    /// The former hand-off: the pixels are encoded as a PNG without a pHYs
    /// chunk and leptonica decodes it again. Kept as the reference the raw
    /// hand-off is proven identical against.
    PngRoundTrip,
}

/// Tesseract's plain text and TSV for one image, read without any parsing.
#[cfg(feature = "leptess-backend")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeptessRawOutput {
    pub utf8_text: String,
    pub tsv: String,
}

#[cfg(feature = "leptess-backend")]
pub struct LeptessOcrEngine {
    inner: leptess::tesseract::TessApi,
    /// Mirrors upstream `SetLanguage`: `ParseHOcrText` reads the selected
    /// affix language for the zhCN word-join rule.
    language: String,
    /// Reused RGBA rows for BGRA images (Tesseract reads 32-bit pixels as
    /// R, G, B, A).
    rgba_scratch: Vec<u8>,
}

#[cfg(feature = "leptess-backend")]
impl LeptessOcrEngine {
    pub fn new(profile: &OcrProfile) -> Result<Self, OcrError> {
        let tessdata_string = profile
            .tessdata_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        let tessdata_path = tessdata_string.as_deref();
        let mut inner =
            leptess::tesseract::TessApi::new(tessdata_path, profile.tesseract_language()).map_err(
                |error| OcrError::Backend(format!("failed to initialize leptess: {error}")),
            )?;
        configure_tesseract_debug_sink(&mut inner)?;
        Ok(Self {
            inner,
            language: profile.language.clone(),
            rgba_scratch: Vec::new(),
        })
    }

    /// Recognizes `image` through the given hand-off and returns
    /// Tesseract's text and TSV verbatim. The equivalence check between the
    /// two hand-offs reads through this.
    pub fn read_raw_output(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
        handoff: LeptessImageHandoff,
    ) -> Result<LeptessRawOutput, OcrError> {
        self.prepare_page(image, options, handoff)?;
        let utf8_text = self
            .inner
            .get_utf8_text()
            .map_err(|error| OcrError::Backend(format!("failed to read OCR text: {error}")))?;
        let tsv = self
            .inner
            .get_tsv_text(0)
            .map_err(structured_output_error)?;
        Ok(LeptessRawOutput { utf8_text, tsv })
    }

    /// Sets the page segmentation mode, the image and the source resolution.
    fn prepare_page(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
        handoff: LeptessImageHandoff,
    ) -> Result<(), OcrError> {
        if image.width() == 0 || image.height() == 0 {
            return Err(OcrError::InvalidImage(
                "OCR image width and height must be non-zero".to_string(),
            ));
        }

        set_tesseract_variable(
            &mut self.inner,
            leptess::Variable::TesseditPagesegMode,
            options.page_seg_mode.tesseract_value(),
        )
        .map_err(|error| OcrError::Backend(format!("failed to set OCR PSM: {error}")))?;
        // Upstream saves the `Image<Gray, byte>` area as an 8-bit bitmap and
        // lets Tesseract read that (OcrHandler.ConvertToText); the gray
        // payload becomes the same 8bpp gray pix either way.
        match handoff {
            LeptessImageHandoff::RawPixels => self.set_raw_image(image)?,
            LeptessImageHandoff::PngRoundTrip => {
                let png = match &image.pixels {
                    OcrImagePixels::Bgra(frame) => d4lc_debug::encode_png_rgba_from_bgra(frame),
                    OcrImagePixels::Gray(gray) => d4lc_debug::encode_gray_png(gray),
                }
                .map_err(|error| {
                    OcrError::InvalidImage(format!("failed to encode OCR image: {error}"))
                })?;
                let pix = leptess::leptonica::pix_read_mem(&png).map_err(|error| {
                    OcrError::Backend(format!("failed to set OCR image: {error}"))
                })?;
                self.inner.set_image(&pix);
            }
        }
        // Same rule as `LepTess::set_fallback_source_resolution(300)`: neither
        // hand-off carries a resolution, so Tesseract gets 300 dpi.
        let resolution = self.inner.get_source_y_resolution();
        if !(leptess::tesseract::MIN_CREDIBLE_RESOLUTION
            ..=leptess::tesseract::MAX_CREDIBLE_RESOLUTION)
            .contains(&resolution)
        {
            self.inner.set_source_resolution(300);
        }
        Ok(())
    }

    fn set_raw_image(&mut self, image: &OcrImage) -> Result<(), OcrError> {
        let (width, height) = (image.width(), image.height());
        let pixel_count = usize::try_from(width)
            .ok()
            .zip(usize::try_from(height).ok())
            .and_then(|(width, height)| width.checked_mul(height))
            .ok_or_else(|| OcrError::InvalidImage("OCR image size overflow".to_string()))?;
        let (bytes, bytes_per_pixel): (&[u8], i32) = match &image.pixels {
            OcrImagePixels::Gray(gray) => {
                if gray.pixels.len() != pixel_count {
                    return Err(OcrError::InvalidImage(format!(
                        "gray OCR image has {} bytes but dimensions require {pixel_count}",
                        gray.pixels.len()
                    )));
                }
                (&gray.pixels, 1)
            }
            OcrImagePixels::Bgra(frame) => {
                bgra_rows_to_rgba(frame, &mut self.rgba_scratch)?;
                (&self.rgba_scratch, 4)
            }
        };
        // tesseract-plumbing multiplies height * bytes_per_line in C int.
        let c_int_of = |value: u32| i32::try_from(value).ok();
        let dimensions = c_int_of(width).zip(c_int_of(height)).and_then(|(w, h)| {
            let bytes_per_line = w.checked_mul(bytes_per_pixel)?;
            h.checked_mul(bytes_per_line)?;
            Some((w, h, bytes_per_line))
        });
        let Some((c_width, c_height, bytes_per_line)) = dimensions else {
            return Err(OcrError::InvalidImage(format!(
                "OCR image {width}x{height} exceeds Tesseract's image size limit"
            )));
        };
        self.inner
            .raw
            .set_image(bytes, c_width, c_height, bytes_per_pixel, bytes_per_line)
            .map_err(|error| OcrError::Backend(format!("failed to set OCR image: {error}")))
    }
}

/// Copies the visible BGRA rows (stride padding dropped) into `rgba` as
/// R, G, B, A — the byte order the PNG hand-off wrote.
#[cfg(feature = "leptess-backend")]
fn bgra_rows_to_rgba(frame: &CaptureFrame, rgba: &mut Vec<u8>) -> Result<(), OcrError> {
    let invalid = |message: &str| OcrError::InvalidImage(message.to_string());
    let width = usize::try_from(frame.width).map_err(|_| invalid("OCR image width overflow"))?;
    let height = usize::try_from(frame.height).map_err(|_| invalid("OCR image height overflow"))?;
    let row_len = width
        .checked_mul(4)
        .ok_or_else(|| invalid("OCR image row length overflow"))?;
    if frame.stride < row_len {
        return Err(invalid("BGRA OCR image stride is shorter than its row"));
    }
    rgba.clear();
    rgba.reserve(row_len.saturating_mul(height));
    for y in 0..height {
        let row = y
            .checked_mul(frame.stride)
            .and_then(|start| frame.pixels_bgra.get(start..start.checked_add(row_len)?))
            .ok_or_else(|| invalid("BGRA OCR image buffer is shorter than its dimensions"))?;
        for pixel in row.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    Ok(())
}

#[cfg(feature = "leptess-backend")]
fn set_tesseract_variable(
    api: &mut leptess::tesseract::TessApi,
    name: leptess::Variable,
    value: &str,
) -> Result<(), String> {
    let value = std::ffi::CString::new(value).map_err(|error| error.to_string())?;
    api.raw
        .set_variable(name.as_cstr(), &value)
        .map_err(|_| "Tess set_variable returned false".to_string())
}

#[cfg(feature = "leptess-backend")]
fn configure_tesseract_debug_sink(api: &mut leptess::tesseract::TessApi) -> Result<(), OcrError> {
    set_tesseract_variable(
        api,
        leptess::Variable::DebugFile,
        tesseract_debug_file_sink(),
    )
    .map_err(|error| OcrError::Backend(format!("failed to configure OCR debug sink: {error}")))
}

#[cfg(any(feature = "leptess-backend", test))]
fn tesseract_debug_file_sink() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

#[cfg(feature = "leptess-backend")]
impl OcrEngine for LeptessOcrEngine {
    fn recognize_lines(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Ok(self.recognize_structured(image, options)?.text)
    }

    fn recognize_structured(
        &mut self,
        image: &OcrImage,
        options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        self.prepare_page(image, options, LeptessImageHandoff::RawPixels)?;
        let raw_text = self
            .inner
            .get_utf8_text()
            .map_err(|error| OcrError::Backend(format!("failed to read OCR text: {error}")))?;

        if !options.collect_tsv {
            return Ok(OcrStructuredOutput {
                text: raw_text_to_ocr_text(&raw_text, image.width(), image.height()),
                tsv: None,
            });
        }

        // A structured-output read failure is a failed recognition: the
        // fallback line grouping below has no geometry and must never pass as
        // the normal structured result.
        let tsv = self
            .inner
            .get_tsv_text(0)
            .map_err(structured_output_error)?;
        let parsed = if options.segment_kind == OcrSegmentKind::UpperTooltip {
            // Upstream ConvertToTextUpperTooltipSection returns
            // ParseHOcrText(page.HOcrText()) verbatim; the plain Tesseract
            // page text is never used for this segment.
            parse_upper_tooltip_text(&tsv, &self.language, &raw_text)
        } else {
            let mut parsed = parse_tesseract_tsv_lines(&tsv, &raw_text);
            parsed.raw_text = upstream_convert_to_text(&parsed.lines, &raw_text);
            parsed
        };
        Ok(OcrStructuredOutput {
            text: parsed,
            tsv: Some(tsv),
        })
    }
}

#[cfg(test)]
mod tests;
