//! The bundled UI catalogues switch at runtime.
//!
//! `@tr` text is compiled together with the generated catalogues
//! (`translations/<culture>/LC_MESSAGES/d4lc-app.po`); picking a language
//! (`slint::select_bundled_translation`, what the settings selector calls)
//! must relabel a live window without recreating it, fall back to English for
//! strings upstream has no translation for, and refuse a language that is not
//! bundled. The debug screen showcase stands in for the app window: it is
//! built from the real screen with the same compiler settings.
//!
//! Own test binary on purpose: Slint installs one platform per process.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Once;

use i_slint_backend_testing::{ElementHandle, ElementRoot};
use slint::ComponentHandle;

#[rustfmt::skip]
#[allow(unused_qualifications, clippy::unwrap_used, clippy::expect_used)]
mod showcase {
    include!(concat!(env!("OUT_DIR"), "/components_showcase.rs"));
}

slint::slint! {
    import { NumberText } from "../ui/components/number_text.slint";

    export component NumberProbe inherits Window {
        in property <float> sample;
        out property <string> text: NumberText.format(sample);
    }
}

fn install_testing_backend() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .expect("the translation test needs the testing backend");
    });
}

fn has_label(app: &impl ElementRoot, label: &str) -> bool {
    ElementHandle::find_by_accessible_label(app, label).count() > 0
}

/// One test body: the testing backend binds its platform to the thread that
/// installed it.
#[test]
fn selecting_a_bundled_language_relabels_the_live_window() {
    install_testing_backend();
    let app = showcase::DebugScreenShowcase::new().expect("DebugScreenShowcase::new failed");
    app.show().expect("DebugScreenShowcase::show failed");

    // The window picks a language from the desktop locale when it is created;
    // the app always selects explicitly, and so does this test.
    slint::select_bundled_translation("en-US").expect("en-US is bundled");
    assert!(has_label(&app, "Top Most"));

    // One caption, one upstream translation per language.
    for (culture, translated) in [
        ("fr-FR", "Au dessus"),
        ("es-ES", "Ventana Principal"),
        ("zh-CN", "最上层"),
    ] {
        slint::select_bundled_translation(culture).expect("bundled");
        assert!(has_label(&app, translated), "{culture}: {translated:?}");
        assert!(
            !has_label(&app, "Top Most"),
            "{culture}: the English caption must be replaced"
        );
    }

    // Upstream's German resources have no value for this key: English stays,
    // as `TranslationSource` falls back to the neutral resource.
    slint::select_bundled_translation("de-DE").expect("bundled");
    assert!(has_label(&app, "Top Most"));

    slint::select_bundled_translation("en-US").expect("bundled");
    assert!(has_label(&app, "Top Most"));

    // Picking a bundled language also switches Slint's decimal separator:
    // a float bound straight to text shows 0.05 as `0,05` in German. The
    // spin boxes format their numbers themselves (`NumberText`), so the debug
    // screen's 0.05 keeps its point in every language.
    for culture in ["en-US", "de-DE", "es-ES", "fr-FR", "zh-CN"] {
        slint::select_bundled_translation(culture).expect("bundled");
        assert!(has_label(&app, "0.05"), "{culture}: en-US number format");
        assert!(!has_label(&app, "0,05"), "{culture}: no locale separator");
    }
    slint::select_bundled_translation("en-US").expect("bundled");

    // The formatter itself: en-US point, no trailing zeros, no separator for
    // whole numbers, sign kept.
    let probe = NumberProbe::new().expect("NumberProbe::new failed");
    for (value, text) in [
        (0.0, "0"),
        (5.0, "5"),
        (255.0, "255"),
        (0.05, "0.05"),
        (0.5, "0.5"),
        (12.25, "12.25"),
        (0.125, "0.125"),
        (1.2, "1.2"),
        (-3.0, "-3"),
        (-0.75, "-0.75"),
        (1000.0, "1000"),
    ] {
        slint::select_bundled_translation("de-DE").expect("bundled");
        probe.set_sample(value);
        assert_eq!(probe.get_text(), text, "{value}");
    }
    slint::select_bundled_translation("en-US").expect("bundled");

    // A language that is not bundled is refused. Upstream's `es-MX` and
    // `zh-TW` cultures are commented out of the selector
    // (`SettingsViewModel.cs:405,415`) and have no resources.
    for culture in ["es-MX", "zh-TW", "klingon"] {
        assert!(
            slint::select_bundled_translation(culture).is_err(),
            "{culture} must not be selectable"
        );
    }
}
