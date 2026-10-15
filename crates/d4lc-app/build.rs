#[allow(clippy::expect_used)]
fn main() {
    // Build scripts are excluded from workspace lint policy; keep a hard failure if Slint UI generation fails.
    // main_window.slint imports all screens via `import { ... } from "...";` so a single
    // compilation pass is sufficient.
    //
    // Element debug info (element ids + type names) is what the headless
    // real-input GUI test harness (`src/testing_harness.rs`) uses to find
    // buttons and inputs and to drive them through the testing backend.
    // Cargo gives a build script no way to detect "this is a `cargo test`
    // build" (PROFILE is `debug` for both `cargo build` and `cargo test`),
    // so the gate is the profile: release — the shipping artifact — never
    // embeds the metadata, while dev-profile builds (test runs included) do.
    let debug_info = std::env::var("PROFILE").as_deref() != Ok("release");
    let config = ui_configuration(debug_info);
    slint_build::compile_with_config("ui/main_window.slint", config).expect("Slint build failed");

    // The debug screen consumes the reusable building blocks (`ui/components/*`)
    // itself, so the shipping compile above sees them. The headless
    // building-block contract (`tests/components_contract.rs`) additionally
    // measures their real element tree, which needs the same debug info, so
    // compile the test-only showcase into OUT_DIR for it.
    // `compile_with_output_path` does
    // not touch SLINT_INCLUDE_GENERATED, so the main module above stays the one
    // `slint::include_modules!()` picks up.
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let showcase_config = ui_configuration(debug_info);
    let dependencies = slint_build::compile_with_output_path(
        manifest_dir.join("tests/components_showcase.slint"),
        out_dir.join("components_showcase.rs"),
        showcase_config,
    )
    .expect("Slint component showcase build failed");
    for dependency in dependencies {
        println!("cargo:rerun-if-changed={}", dependency.display());
    }
}

/// The compiler settings both UI roots share.
///
/// `translations/<lang>/LC_MESSAGES/d4lc-app.po` are bundled into the binary
/// (generated from the upstream resx files by `cargo xtask
/// import-upstream-locales`). The catalogues carry no message contexts, so the
/// default per-component context is switched off to make `@tr` msgids match
/// them as written.
fn ui_configuration(debug_info: bool) -> slint_build::CompilerConfiguration {
    slint_build::CompilerConfiguration::new()
        .with_debug_info(debug_info)
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
}
