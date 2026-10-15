use super::*;
use crate::structured_output_error;
use crate::test_support::tiny_image;
use d4lc_core::{OcrBoundingBoxSource, OcrLine, OcrWord, Rect};
use std::sync::atomic::AtomicUsize;

struct FakeOcrEngine {
    id: usize,
}

impl OcrEngine for FakeOcrEngine {
    fn recognize_lines(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Ok(OcrText {
            raw_text: format!("engine-{}", self.id),
            lines: vec![OcrLine {
                text: format!("engine-{}", self.id),
                rect: Rect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
                words: vec![OcrWord {
                    text: format!("engine-{}", self.id),
                    rect: Rect {
                        x: 1,
                        y: 2,
                        width: 3,
                        height: 4,
                    },
                }],
            }],
            bounding_box_source: OcrBoundingBoxSource::SyntheticFullImage,
        })
    }
}

struct StructuredFailureEngine;

impl OcrEngine for StructuredFailureEngine {
    fn recognize_lines(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrText, OcrError> {
        Ok(OcrText {
            raw_text: "text-only fallback".to_string(),
            lines: Vec::new(),
            bounding_box_source: OcrBoundingBoxSource::Unavailable,
        })
    }

    fn recognize_structured(
        &mut self,
        _image: &OcrImage,
        _options: &OcrOptions,
    ) -> Result<OcrStructuredOutput, OcrError> {
        Err(structured_output_error("invalid utf-8 sequence"))
    }
}

#[test]
fn structured_output_failure_reaches_the_registry_caller() {
    let registry =
        OcrEngineRegistry::new(|_profile| Ok(Box::new(StructuredFailureEngine) as BoxedOcrEngine));

    let result = registry.recognize_structured(
        OcrProfile::new("enUS"),
        &tiny_image(),
        &OcrOptions::default(),
    );

    match result {
        Err(OcrError::Backend(message)) => assert!(
            message.contains("failed to read OCR structured output (TSV)"),
            "{message}"
        ),
        other => panic!("expected the structured-output failure, got {other:?}"),
    }
}

#[test]
fn engines_without_structured_output_keep_the_parsed_text_and_report_no_tsv() {
    let registry =
        OcrEngineRegistry::new(|_profile| Ok(Box::new(FakeOcrEngine { id: 1 }) as BoxedOcrEngine));

    let output = registry
        .recognize_structured(
            OcrProfile::new("enUS"),
            &tiny_image(),
            &OcrOptions::default(),
        )
        .unwrap_or_else(|error| panic!("structured recognition failed: {error}"));

    assert_eq!(output.text.raw_text, "engine-1");
    assert_eq!(output.tsv, None);
}

#[test]
fn engine_pool_reuses_returned_engine() {
    let next_id = Arc::new(AtomicUsize::new(1));
    let ids = Arc::clone(&next_id);
    let registry = OcrEngineRegistry::new(move |_profile| {
        let id = ids.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeOcrEngine { id }))
    });
    let profile = OcrProfile::new("enUS");
    let image = tiny_image();
    let options = OcrOptions::default();

    let first = registry
        .recognize_lines(profile.clone(), &image, &options)
        .unwrap_or_else(|error| panic!("first OCR failed: {error}"));
    let second = registry
        .recognize_lines(profile.clone(), &image, &options)
        .unwrap_or_else(|error| panic!("second OCR failed: {error}"));
    let pool = registry
        .pool_for(profile)
        .unwrap_or_else(|error| panic!("pool lookup failed: {error}"));
    let stats = pool
        .stats()
        .unwrap_or_else(|error| panic!("pool stats failed: {error}"));

    assert_eq!(first.raw_text, "engine-1");
    assert_eq!(second.raw_text, "engine-1");
    assert_eq!(pool.available_engines(), Ok(1));
    assert_eq!(stats.initialized, 1);
    assert_eq!(stats.reused, 1);
    assert_eq!(stats.returned, 2);
    assert_eq!(stats.available, 1);
    assert_eq!(next_id.load(Ordering::SeqCst), 2);
}

#[test]
fn language_switch_clears_previous_pool_and_returns_new_profile() {
    let next_id = Arc::new(AtomicUsize::new(1));
    let ids = Arc::clone(&next_id);
    let registry = OcrEngineRegistry::new(move |_profile| {
        let id = ids.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeOcrEngine { id }))
    });
    let image = tiny_image();
    let options = OcrOptions::default();
    let previous = OcrProfile::new("enUS");

    registry
        .recognize_lines(previous.clone(), &image, &options)
        .unwrap_or_else(|error| panic!("seed OCR failed: {error}"));
    assert_eq!(
        registry
            .pool_for(previous.clone())
            .unwrap_or_else(|error| panic!("pool lookup failed: {error}"))
            .available_engines(),
        Ok(1)
    );

    let next = registry
        .switch_language(&previous, "frFR")
        .unwrap_or_else(|error| panic!("language switch failed: {error}"));

    assert_eq!(next.language, "frFR");
    assert_eq!(
        registry
            .pool_for(previous)
            .unwrap_or_else(|error| panic!("previous pool lookup failed: {error}"))
            .available_engines(),
        Ok(0)
    );
}
