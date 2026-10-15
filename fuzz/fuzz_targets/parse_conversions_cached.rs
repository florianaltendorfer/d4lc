//! Fuzz target for the cached fuzzy conversion entry points.
//!
//! Contract: for any UTF-8 OCR garbage, `convert_to_affix`,
//! `convert_to_affix_multiline`, `convert_to_aspect`, `convert_to_unique`,
//! and `convert_to_item_type` (upstream `OcrHandler.ConvertTo*` and the
//! `TextTo*Cached` lookups) must return without panicking. libFuzzer records
//! any panic, overflow-check failure, or abort as a crash.

#![no_main]

use std::path::PathBuf;
use std::sync::LazyLock;

use d4lc_assets::AssetRepository;
use d4lc_fuzzy::FuzzyIndexes;
use libfuzzer_sys::fuzz_target;

/// enUS fuzzy indexes loaded once from the workspace's imported upstream
/// assets. The first execution pays the one-time load cost.
static INDEXES: LazyLock<FuzzyIndexes> = LazyLock::new(|| {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("fuzz crate must live inside the d4lc workspace")
        .to_path_buf();
    let repo = AssetRepository::new(workspace_root);
    FuzzyIndexes::load(&repo, "enUS").expect("enUS fuzzy indexes must load from workspace assets")
});

fuzz_target!(|data: &[u8]| {
    // The conversion entry points take `&str` (OCR output is UTF-8), so
    // non-UTF-8 bytes are outside the fuzzed input domain.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let indexes = &*INDEXES;

    let _ = d4lc_parse::convert_to_affix(text, indexes);
    let _ = d4lc_parse::convert_to_affix_multiline(text, indexes);
    let _ = d4lc_parse::convert_to_aspect(text, indexes);
    let _ = d4lc_parse::convert_to_unique(text, indexes);
    let _ = d4lc_parse::convert_to_item_type(text, indexes);
});
