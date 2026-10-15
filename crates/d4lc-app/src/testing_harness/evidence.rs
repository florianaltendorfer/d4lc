use super::*;

// ── Screenshot artifacts ──────────────────────────────────────────────────

/// Assert the bytes are a non-empty PNG and return `(width, height)` from
/// the IHDR chunk. Comparisons are programmatic; visual review is not part
/// of the test.
pub(super) fn assert_png_and_dimensions(png: &[u8]) -> (u32, u32) {
    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert!(
        png.len() > 24 && png[..8] == PNG_MAGIC && &png[12..16] == b"IHDR",
        "screenshot is not a non-empty PNG ({} bytes)",
        png.len()
    );
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    (width, height)
}

/// Write a screenshot under `debug-output/pilots/<scenario>/` in the
/// workspace.
fn write_evidence_screenshot(scenario: &str, name: &str, png: &[u8]) {
    let dir = workspace_root().join("debug-output/pilots").join(scenario);
    std::fs::create_dir_all(&dir).expect("create the evidence directory");
    let path = dir.join(name);
    std::fs::write(&path, png).expect("write screenshot");
    eprintln!("[{scenario}] screenshot written: {}", path.display());
}

/// Write a screenshot of the basic harness scenario.
pub(super) fn write_screenshot(name: &str, png: &[u8]) {
    write_evidence_screenshot("gui-harness", name, png);
}

pub(super) fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root missing")
        .to_path_buf()
}

/// Take a window screenshot, check it, and write it as evidence of
/// `scenario`.
pub(super) fn take_evidence_screenshot(
    client: &McpClient,
    window: McpHandle,
    scenario: &str,
    name: &str,
) {
    let png = client
        .take_screenshot(window)
        .unwrap_or_else(|error| harness_panic(&format!("{scenario} screenshot"), &error));
    assert_png_and_dimensions(&png);
    write_evidence_screenshot(scenario, name, &png);
}
