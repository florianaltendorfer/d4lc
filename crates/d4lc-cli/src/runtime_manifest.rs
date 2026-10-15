//! Integrity guard for the reference fixtures.
//!
//! `tests/fixtures/runtime/manifest.json` inventories the fixtures: every case
//! states its origin, the files it consists of with their sha256, and the
//! catalog state the corpus binds to. The guard fails when a declared file
//! goes missing or changes, when a fixture file is not inventoried, and when a
//! case claims expectations its own files do not back.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::upstream::{read_upstream_lock, sha256_hex};

const MANIFEST_PATH: &str = "tests/fixtures/runtime/manifest.json";
const CORPUS_ROOTS: [&str; 2] = [
    "tests/fixtures/live-frames",
    "crates/d4lc-rules/tests/fixtures/imports",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| panic!("could not resolve the workspace root"))
}

fn manifest() -> Value {
    let path = workspace_root().join(MANIFEST_PATH);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", path.display()))
}

fn array_field<'a>(value: &'a Value, field: &str) -> &'a [Value] {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("{field} must be a JSON array"))
}

fn string_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{field} must be a string"))
}

fn collect_files(directory: &Path, relative_directory: &str, files: &mut BTreeSet<String>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to list {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "failed to read an entry of {}: {error}",
                directory.display()
            )
        });
        let relative = format!(
            "{relative_directory}/{}",
            entry.file_name().to_string_lossy()
        );
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &relative, files);
        } else {
            files.insert(relative);
        }
    }
}

fn png_dimensions(path: &Path) -> (u32, u32) {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert!(
        bytes.len() > 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{} must be a PNG file",
        path.display()
    );
    assert_eq!(
        &bytes[12..16],
        b"IHDR",
        "{} must start with an IHDR chunk",
        path.display()
    );
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (width, height)
}

#[test]
fn runtime_manifest_entries_are_hash_accurate() {
    let root = workspace_root();
    let manifest = manifest();
    let cases = array_field(&manifest, "cases");
    assert!(
        !cases.is_empty(),
        "{MANIFEST_PATH} must declare at least one reference case"
    );

    let mut seen_ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();

    for case in cases {
        let case_id = string_field(case, "case_id");
        assert!(
            seen_ids.insert(case_id.to_string()),
            "{case_id}: duplicate case_id"
        );
        let origin = string_field(case, "origin");
        assert!(
            origin == "synthetic" || origin == "real",
            "{case_id}: origin must be synthetic or real, got {origin}"
        );
        let files = array_field(case, "files");

        match string_field(case, "availability") {
            "present" => {
                assert!(
                    !files.is_empty(),
                    "{case_id}: an available case must declare its files"
                );
                for file in files {
                    let relative = string_field(file, "path");
                    let expected = string_field(file, "sha256");
                    assert!(
                        expected.len() == 64
                            && expected.chars().all(|digit| digit.is_ascii_hexdigit()),
                        "{case_id}: {relative} must declare a 64 character sha256"
                    );
                    assert!(
                        seen_paths.insert(relative.to_string()),
                        "{relative} is declared by more than one case"
                    );
                    let path = root.join(relative);
                    let bytes = fs::read(&path).unwrap_or_else(|error| {
                        panic!(
                            "{case_id}: declared file {} does not resolve: {error}",
                            path.display()
                        )
                    });
                    assert_eq!(
                        sha256_hex(&bytes),
                        expected,
                        "{case_id}: sha256 mismatch for {relative}"
                    );
                }

                if let Some(rows) = case
                    .get("expected_affix_rows")
                    .filter(|value| value.is_object())
                {
                    let defined_by = string_field(rows, "defined_by");
                    assert!(
                        files
                            .iter()
                            .any(|file| string_field(file, "path") == defined_by),
                        "{case_id}: expected rows must be defined by one of the case's own files, got {defined_by}"
                    );
                    let row_count = rows
                        .get("row_count")
                        .and_then(Value::as_u64)
                        .unwrap_or_else(|| {
                            panic!("{case_id}: expected_affix_rows needs a row_count")
                        });
                    let text = fs::read_to_string(root.join(defined_by)).unwrap_or_else(|error| {
                        panic!("{case_id}: failed to read {defined_by}: {error}")
                    });
                    let payload: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
                        panic!("{case_id}: {defined_by} is not valid JSON: {error}")
                    });
                    let affixes = payload
                        .get("affix_preset")
                        .and_then(|preset| preset.get("affixes"))
                        .and_then(Value::as_array)
                        .unwrap_or_else(|| {
                            panic!("{case_id}: {defined_by} must carry affix_preset.affixes")
                        });
                    assert_eq!(
                        row_count as usize,
                        affixes.len(),
                        "{case_id}: row_count must mirror {defined_by}"
                    );
                }

                if string_field(case, "kind") == "capture_frame" {
                    let selection = case
                        .get("expected_tooltip_selection")
                        .filter(|value| value.is_object())
                        .unwrap_or_else(|| {
                            panic!("{case_id}: a capture frame must declare its expected tooltip selection")
                        });
                    assert!(
                        !string_field(selection, "oracle").is_empty(),
                        "{case_id}: the tooltip selection needs a test oracle"
                    );
                    let pixels = case
                        .get("window_geometry")
                        .and_then(|geometry| geometry.get("capture_pixels"))
                        .and_then(Value::as_array)
                        .unwrap_or_else(|| {
                            panic!("{case_id}: a capture frame must declare capture_pixels")
                        });
                    let width = pixels
                        .first()
                        .and_then(Value::as_u64)
                        .unwrap_or_else(|| panic!("{case_id}: capture_pixels needs a width"));
                    let height = pixels
                        .get(1)
                        .and_then(Value::as_u64)
                        .unwrap_or_else(|| panic!("{case_id}: capture_pixels needs a height"));
                    let relative = string_field(&files[0], "path");
                    assert_eq!(
                        (width as u32, height as u32),
                        png_dimensions(&root.join(relative)),
                        "{case_id}: capture_pixels must match the recorded frame {relative}"
                    );
                }
            }
            other => panic!("{case_id}: availability must be present, got {other}"),
        }
    }

    let binding = manifest
        .get("catalog_binding")
        .unwrap_or_else(|| panic!("catalog_binding must be present"));
    assert_eq!(
        string_field(binding, "gate_success_line"),
        "UPSTREAM_ASSET_PARITY_OK=true",
        "the manifest must bind to the line the local asset parity gate prints"
    );
    let upstream_lock = read_upstream_lock(&root)
        .unwrap_or_else(|error| panic!("failed to read upstream.lock: {error}"));
    assert_eq!(
        string_field(binding, "upstream_lock_commit"),
        upstream_lock.last_reviewed_commit.as_str(),
        "the corpus must bind to the catalog state of the current upstream.lock"
    );
}

#[test]
fn runtime_manifest_inventories_every_corpus_file_exactly_once() {
    let root = workspace_root();
    let mut on_disk = BTreeSet::new();
    for corpus_root in CORPUS_ROOTS {
        collect_files(&root.join(corpus_root), corpus_root, &mut on_disk);
    }

    let manifest = manifest();
    let mut declared = BTreeSet::new();
    for case in array_field(&manifest, "cases") {
        for file in array_field(case, "files") {
            declared.insert(string_field(file, "path").to_string());
        }
    }

    assert_eq!(
        declared, on_disk,
        "the reference corpus and {MANIFEST_PATH} must inventory the same files"
    );
}
