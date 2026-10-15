use super::*;
use crate::ocr_parity::detection::detect_upstream_markers_notifying;
use crate::ocr_parity::test_support::{reference_capture, workspace_root};
use d4lc_core::Rect;

/// A tempered affix rolled greater carries an orange-rimmed star the
/// upstream preset has no template for: on the recorded capture the upstream
/// templates alone find four affix markers and miss "+12.0% Attack
/// Speed"; the d4lc supplement adds that fifth marker, as a Greater
/// marker by its upstream file-name prefix.
#[test]
fn tempered_greater_affix_marker_is_found_through_the_supplement() {
    let repo = AssetRepository::new(workspace_root());
    let loaded = repo
        .load_preset_templates("1080p_SMF")
        .unwrap_or_else(|error| panic!("{error}"));
    let settings = d4lc_config::AppSettings::default();
    let frame = reference_capture("tooltip-tempered-greater-affix.png");
    let affix_names = |templates: &[MarkerTemplate]| {
        detect_upstream_markers_notifying(&frame, templates, 70, 255, &settings, |_| {})
            .unwrap_or_else(|error| panic!("{error}"))
            .affix_markers
            .into_iter()
            .map(|marker| (marker.name, marker.rect))
            .collect::<Vec<_>>()
    };

    let mut templates = load_marker_templates(&repo, &loaded.templates, 70, 255)
        .unwrap_or_else(|error| panic!("{error}"));
    let upstream_only = affix_names(&templates);
    assert_eq!(upstream_only.len(), 4, "{upstream_only:?}");

    add_supplement_marker_templates(&mut templates, &repo, "1080p_SMF", 70, 255)
        .unwrap_or_else(|error| panic!("{error}"));
    let with_supplement = affix_names(&templates);
    assert_eq!(with_supplement.len(), 5, "{with_supplement:?}");
    let tempered_greater = with_supplement
        .iter()
        .filter(|(name, _)| name == "dot-affixes_greater_tempered.png")
        .map(|(_, rect)| *rect)
        .collect::<Vec<_>>();
    assert_eq!(
        tempered_greater,
        vec![Rect {
            x: 18,
            y: 496,
            width: 16,
            height: 16
        }]
    );
    assert_eq!(
        d4lc_ocr::affix_type_from_marker_name("dot-affixes_greater_tempered.png"),
        d4lc_ocr::AffixType::Greater
    );

    // A second pass never adds the supplement twice.
    let count = templates.len();
    add_supplement_marker_templates(&mut templates, &repo, "1080p_SMF", 70, 255)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(templates.len(), count);
}

/// A PNG whose `IHDR` claims `width` × `height` at `bit_depth` /
/// `color_type`, followed by an empty `IDAT`: the header alone decides
/// whether the decoder may allocate the pixel buffer.
fn png_header_only(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(
            &u32::try_from(data.len())
                .unwrap_or_else(|error| panic!("{error}"))
                .to_be_bytes(),
        );
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[bit_depth, color_type, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(
        &mut png,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    chunk(&mut png, b"IEND", &[]);
    png
}

/// The decoder's own message: the text after `failed to decode {path}: `.
fn decode_error(path: &std::path::Path) -> String {
    let Err(error) = load_template_gray_image(path) else {
        panic!("{} must be refused", path.display());
    };
    let prefix = format!("failed to decode {}: ", path.display());
    error
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("unexpected error shape: {error}"))
        .to_string()
}

#[test]
fn template_decode_accepts_real_preset_images_and_enforces_the_limits() {
    let preset = workspace_root().join("assets/original/systempresets/1080p_SMF");
    let mut decoded = 0;
    for entry in std::fs::read_dir(&preset).unwrap_or_else(|error| panic!("{error}")) {
        let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
        if path.extension().is_some_and(|extension| extension == "png") {
            let image = load_template_gray_image(&path).unwrap_or_else(|error| panic!("{error}"));
            assert!(image.width > 0 && image.height > 0, "{}", path.display());
            assert!(
                image.width <= 180 && image.height <= 180,
                "{}",
                path.display()
            );
            decoded += 1;
        }
    }
    assert!(decoded >= 10, "decoded {decoded} preset PNGs");

    let dir = std::env::temp_dir().join(format!("d4lc-template-decode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    let max = TEMPLATE_IMAGE_MAX_SIDE;
    // Well-formed PNGs written by the `image` encoder: exactly the side
    // limit decodes, one pixel more is refused before decoding.
    for (name, width, height, accepted) in [
        ("at-limit-wide.png", max, 1, true),
        ("at-limit-tall.png", 1, max, true),
        ("over-limit-wide.png", max + 1, 1, false),
        ("over-limit-tall.png", 1, max + 1, false),
    ] {
        let path = dir.join(name);
        image::GrayImage::from_pixel(width, height, image::Luma([200]))
            .save(&path)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        if accepted {
            let image = load_template_gray_image(&path).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!((image.width, image.height), (width, height), "{name}");
        } else {
            assert_eq!(decode_error(&path), "Image size exceeds limit", "{name}");
        }
    }
    // Inside the side limit, a 16-bit RGBA image of 4096 × 2049 needs more
    // than the 64 MiB allocation cap; the header alone is refused.
    let path = dir.join("over-alloc.png");
    std::fs::write(&path, png_header_only(max, max / 2 + 1, 16, 6))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(decode_error(&path), "Memory limit exceeded");
    let _ = std::fs::remove_dir_all(&dir);
}
