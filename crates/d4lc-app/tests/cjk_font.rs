//! Whether the UI font stack renders Chinese text on this machine.
//!
//! Slint draws text with the system's fonts and finds a fallback font per
//! script; the port bundles none for the UI. Without a CJK font on the system
//! the zh-CN language shows empty boxes, and the same box for every character.
//! The test renders two different Chinese strings and requires different, non-empty
//! pictures. It depends on the machine's fonts (CI images often have none), so
//! it is opt-in: `cargo test -p d4lc-app --test cjk_font -- --ignored`.
//!
//! Own test binary: Slint installs one platform per process.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use slint::ComponentHandle;

slint::slint! {
    export component Probe inherits Window {
        in property <string> sample;
        width: 240px;
        height: 60px;
        background: black;
        Text {
            text: sample;
            color: white;
            font-size: 28px;
        }
    }
}

fn render(sample: &str) -> Vec<u8> {
    let probe = Probe::new().expect("Probe::new failed");
    probe.show().expect("Probe::show failed");
    probe.set_sample(sample.into());
    let buffer = probe.window().take_snapshot().expect("snapshot");
    buffer.as_bytes().to_vec()
}

#[test]
#[ignore = "needs a system CJK font, for example Noto Sans CJK"]
fn the_ui_font_stack_renders_chinese() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("the probe needs the testing backend");

    let first = render("最上层设置");
    let second = render("词缀巅峰符文");
    let lit = |pixels: &[u8]| pixels.chunks(4).filter(|p| p[0] > 0 || p[1] > 0).count();
    assert!(lit(&first) > 200, "Chinese text drew (almost) nothing");
    assert_ne!(
        first, second,
        "two different Chinese strings rendered identically: the font has no CJK glyphs"
    );
}
