use std::process::Command;

#[test]
fn help_version_json_and_usage_errors_use_the_expected_streams_and_exit_codes() {
    for args in [
        vec![],
        vec!["help"],
        vec!["--help"],
        vec!["-h"],
        vec!["--version"],
        vec!["live-loop", "--help"],
        vec!["help", "read-tooltip"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_d4lc"))
            .args(&args)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(!output.stdout.is_empty(), "{args:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    }
    for args in [
        vec!["typo"],
        vec!["asset-report", "--typo"],
        vec!["live-loop", "--frames", "0"],
        vec!["asset-report", "--root"],
        vec!["upstream-changed"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_d4lc"))
            .args(&args)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(message.starts_with("error: "), "{message}");
        assert!(!message.starts_with("error: error:"), "{message}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_d4lc"))
        .arg("asset-languages")
        .output()
        .unwrap_or_else(|error| panic!("asset-languages: {error}"));
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON report: {error}"));
    assert!(report["languages"]
        .as_array()
        .is_some_and(|languages| languages.iter().any(|code| code == "enUS")));
}

/// The packaged launchers export `D4LC_ASSET_ROOT` and keep the caller's
/// working directory: a run then reads the (read-only) bundle and the
/// user's config root and writes only below the working directory.
#[test]
fn a_packaged_run_reads_the_environment_roots_and_writes_below_the_working_directory() {
    use std::os::unix::fs::PermissionsExt as _;

    let base = std::env::temp_dir().join(format!(
        "d4lc-cli-contract-packaged-run-{}",
        std::process::id()
    ));
    let (assets, config, work, home) = (
        base.join("bundle"),
        base.join("config"),
        base.join("work"),
        base.join("home"),
    );
    for dir in [&assets, &config.join("Config"), &work, &home] {
        std::fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    }
    std::fs::write(
        config.join("Config/Settings.json"),
        r#"{ "SelectedSystemPreset": "2160p_SSF", "SelectedAffixLanguage": "deDE" }"#,
    )
    .unwrap_or_else(|error| panic!("settings: {error}"));
    std::fs::set_permissions(&assets, std::fs::Permissions::from_mode(0o555))
        .unwrap_or_else(|error| panic!("chmod: {error}"));
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_d4lc"))
            .args(args)
            .current_dir(&work)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("D4LC_ASSET_ROOT", &assets)
            .env("D4LC_CONFIG_ROOT", &config)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap_or_else(|error| panic!("{args:?}: {error}"))
    };
    let json = |output: &std::process::Output| -> serde_json::Value {
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}"))
    };

    let summary = json(&run(&["manual-diablo-validation-bundle", "--dry-run"]));
    let bundle_dir = std::path::PathBuf::from(summary["bundle_dir"].as_str().unwrap_or_default());
    assert!(
        bundle_dir.starts_with(work.join("debug-output/manual-diablo-iv")),
        "{bundle_dir:?}"
    );
    assert!(bundle_dir.join("bundle-summary.json").is_file());
    let snapshot: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle_dir.join("settings-snapshot.json"))
            .unwrap_or_else(|error| panic!("snapshot: {error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(snapshot["status"], "loaded");
    assert_eq!(snapshot["selected_system_preset"], "2160p_SSF");
    assert_eq!(snapshot["config"]["config_root_source"], "D4LC_CONFIG_ROOT");
    let bundle_entries = std::fs::read_dir(&assets)
        .unwrap_or_else(|error| panic!("{error}"))
        .count();
    assert_eq!(
        bundle_entries, 0,
        "nothing may be written into the asset root"
    );

    let report = json(&run(&["settings-report"]));
    assert_eq!(report["selected_affix_language"], "deDE");

    let from_env = run(&["asset-presets"]);
    assert_eq!(from_env.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&from_env.stderr).contains(&*assets.to_string_lossy()));
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let flagged = json(&run(&[
        "asset-presets",
        "--root",
        &workspace.to_string_lossy(),
    ]));
    assert!(flagged["presets"]
        .as_array()
        .is_some_and(|presets| !presets.is_empty()));

    std::fs::set_permissions(&assets, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("chmod: {error}"));
    std::fs::remove_dir_all(&base).unwrap_or_else(|error| panic!("cleanup: {error}"));
}
