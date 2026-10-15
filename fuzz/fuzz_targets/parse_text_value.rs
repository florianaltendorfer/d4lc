//! Fuzz target for the pure-text OCR conversion entry points.
//!
//! Contract: for any UTF-8 input, `d4lc_parse::text_to_affix_value` (upstream
//! `OcrHandler.TextToAffixValue`) and `d4lc_parse::convert_to_item_power`
//! (upstream `OcrHandler.ConvertToPower`) must return without panicking.
//! libFuzzer records any panic, overflow-check failure, or abort as a crash;
//! reaching the end of this function is the assertion.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The conversion entry points take `&str` (OCR output is UTF-8), so
    // non-UTF-8 bytes are outside the fuzzed input domain.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = d4lc_parse::text_to_affix_value(text);
    let _ = d4lc_parse::convert_to_item_power(text);
});
