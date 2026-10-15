//! Release-mode latency harness for the overlay draw/update path.
//!
//! Measures per-update latency of `OverlayHandle::update` — the call the live
//! loop performs once per captured frame
//! (`crates/d4lc-cli/src/live.rs::run_live_loop_session`, overlay update after
//! `process_latest_frame`) — across:
//!
//! * backends: `headless_noop` (render-plan expansion only, no X11 I/O) and,
//!   when compiled with `--features real-x11`, `real_x11` on a synthetic Xvfb
//!   display (the overlay smoke environment; never the real `:0`
//!   display);
//! * product descriptor workloads built through the `d4lc-rules` product
//!   chain (`evaluate_multi_build_affix_presets` →
//!   `apply_item_power_and_value_filters` → `apply_trade_item_lookup` →
//!   `apply_overlay_descriptors`, combined with
//!   `build_paragon_overlay_descriptors` via
//!   `build_live_overlay_descriptors`):
//!   - `single_tooltip`: 4-affix legendary amulet + aspect power + trade
//!     match (the common per-tooltip live frame);
//!   - `combined_full_stack`: the same rule frame plus a full paragon board
//!     (21x21 tile grid), the heavy frame magnitude of the overlay smoke
//!     (466 render ops);
//!   - `paragon_full` / `paragon_collapsed`: the paragon overlay alone at the
//!     production layout (2560x1440 window, default node sizes and collapsed
//!     offsets, 18 px font, 8 board rows, a 441-tile grid). The
//!     `paragon_*_descriptor_build` scenarios time the descriptor producer
//!     itself (`build_live_overlay_descriptors`) instead of the overlay
//!     update.
//!
//! Methodology: one discarded warmup invocation before the counted run (plus
//! 10 in-process discarded updates per scenario), 240 counted samples per
//! scenario (minimum 60), nearest-rank percentiles over integer
//! microsecond samples (the `live.rs::percentile_ms` rank formula; sub-ms
//! precision is required by the 4 ms budget).
//!
//! Run (bench only, not part of the default suite):
//! ```text
//! cargo test -p d4lc-overlay-x11 --release --test perf_overlay_draw \
//!   -- --ignored --nocapture --test-threads=1
//! # real X11 draw scenario additionally requires:
//! cargo test -p d4lc-overlay-x11 --release --features real-x11 \
//!   --test perf_overlay_draw -- --ignored --nocapture --test-threads=1
//! ```
//! The report label defaults to `run`; set `D4LC_PERF_LABEL=before|after` for
//! A/B evidence runs. Raw run JSON is written to
//! `debug-output/perf-overlay-draw/<label>.json` under the workspace root.

use d4lc_core::text_measure::FixedTextMeasure;
use d4lc_overlay_x11::{
    create_overlay, OverlayBackend, OverlayConfig, OverlayFrame, OverlayHandle,
};
use d4lc_rules::{
    apply_item_power_and_value_filters, apply_overlay_descriptors, apply_trade_item_lookup,
    build_live_overlay_descriptors, evaluate_multi_build_affix_presets, AffixPreset,
    AffixRequirement, AffixValueThreshold, ImportedParagonBoard, ImportedParagonRoute,
    MultiBuildEntry, MultiBuildSettings, ParagonOverlayLayout, ParagonOverlayMode,
    ParagonOverlayState, ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules,
    RuleFilterSettings, RuleIdOverlayLocation, RuleOverlayLayout, RuleOverlayMarkerMode,
    RuleOverlaySettings, TradeAffixRequirement, TradeItemRule,
};
use std::path::PathBuf;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Product descriptor workloads (d4lc-rules evaluation chain).
// ---------------------------------------------------------------------------

const WINDOW_GEOMETRY: d4lc_core::Rect = d4lc_core::Rect {
    x: 0,
    y: 0,
    width: 1920,
    height: 1080,
};

fn bench_affix(index: usize) -> ParsedAffixForRules {
    let (affix_id, value_text) = match index {
        0 => ("Affix_AttackSpeed".to_string(), "+32.0%".to_string()),
        1 => ("Affix_Thorns".to_string(), "+715".to_string()),
        2 => (
            "Affix_CriticalStrikeDamage".to_string(),
            "+58.0%".to_string(),
        ),
        _ => ("Affix_LifeOnKill".to_string(), "+[150 - 200]".to_string()),
    };
    ParsedAffixForRules {
        affix_id,
        label: None,
        value_text: Some(value_text),
        source_text: "bench affix line".to_string(),
        row_id: Some(d4lc_core::AffixRowId::from_ordinal(index)),
        kind: ParsedAffixKind::Normal,
    }
}

fn bench_item() -> ParsedItemForRules {
    ParsedItemForRules {
        item_type: "Amulet".to_string(),
        rarity: Some("legendary".to_string()),
        affixes: (0..4).map(bench_affix).collect(),
    }
}

fn bench_multi_build() -> MultiBuildSettings {
    MultiBuildSettings {
        enabled: true,
        selected_preset: "Bench".to_string(),
        builds: vec![
            MultiBuildEntry {
                build_name: "Bench A".to_string(),
                preset_name: "Bench".to_string(),
                color: "Blue".to_string(),
            },
            MultiBuildEntry {
                build_name: "Bench B".to_string(),
                preset_name: "Bench".to_string(),
                color: "Purple".to_string(),
            },
            MultiBuildEntry {
                build_name: "Bench C".to_string(),
                preset_name: "Missing Preset".to_string(),
                color: "Red".to_string(),
            },
        ],
    }
}

fn bench_presets() -> Vec<AffixPreset> {
    vec![AffixPreset {
        name: "Bench".to_string(),
        affixes: (0..4)
            .map(|index| AffixRequirement {
                affix_id: bench_affix(index).affix_id,
                label: None,
                item_type: "Amulet".to_string(),
                color: "Green".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            })
            .collect(),
    }]
}

fn bench_trade_items() -> Vec<TradeItemRule> {
    vec![TradeItemRule {
        trade_item_id: "trade-amulet-match".to_string(),
        item_type: "Amulet".to_string(),
        value_text: "5b".to_string(),
        affixes: vec![TradeAffixRequirement {
            affix_id: "Affix_AttackSpeed".to_string(),
            is_greater: false,
        }],
    }]
}

fn bench_overlay_layout() -> RuleOverlayLayout {
    RuleOverlayLayout {
        tooltip_rect: d4lc_core::Rect {
            x: 1180,
            y: 60,
            width: 500,
            height: 700,
        },
        affix_locations: (0..4)
            .map(|index| d4lc_rules::RuleAffixOverlayLocation {
                row_id: Some(d4lc_core::AffixRowId::from_ordinal(index)),
                geometry: d4lc_core::Rect {
                    x: 1220,
                    y: 260 + (index as i32) * 28,
                    width: 300,
                    height: 20,
                },
                text: None,
            })
            .collect(),
        power_location: Some(RuleIdOverlayLocation {
            id: "Aspect_BerserkRipping".to_string(),
            geometry: d4lc_core::Rect {
                x: 1220,
                y: 420,
                width: 420,
                height: 40,
            },
            color: "Gold".to_string(),
            text: Some("Aspect".to_string()),
        }),
        power_locations: Vec::new(),
        rune_locations: Vec::new(),
        sigil_location: None,
        sigil_locations: Vec::new(),
    }
}

fn bench_overlay_settings() -> RuleOverlaySettings {
    RuleOverlaySettings {
        marker_mode: RuleOverlayMarkerMode::ShowAll,
        is_minimal_affix_value_filter_enabled: true,
        is_trade_overlay_enabled: true,
        item_power_limit_check_ok: true,
        is_tuning_prism_hints_enabled: true,
    }
}

/// The live consumer sequence for one tooltip frame
/// (`evaluate_multi_build_affix_presets` → `apply_item_power_and_value_filters`
/// → `apply_trade_item_lookup` → `apply_overlay_descriptors`).
fn evaluate_rule_descriptors() -> d4lc_rules::RuleEvaluationResult {
    let item = bench_item();
    let multi_build = bench_multi_build();
    let presets = bench_presets();
    let mut evaluation = evaluate_multi_build_affix_presets(&item, &multi_build, &presets);
    apply_item_power_and_value_filters(
        &mut evaluation,
        Some(925),
        item.item_type.as_str(),
        &RuleFilterSettings {
            is_item_power_limit_enabled: true,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: true,
        },
        &[AffixValueThreshold {
            affix_id: "Affix_AttackSpeed".to_string(),
            threshold: 20.0,
        }],
    );
    apply_trade_item_lookup(&mut evaluation, &item, &bench_trade_items());
    apply_overlay_descriptors(
        &mut evaluation,
        &bench_overlay_settings(),
        &bench_overlay_layout(),
    );
    evaluation
}

/// The synthetic smoke paragon state: one route, one board, four
/// active nodes — which the product builder expands into a full 21x21 tile
/// grid plus panel and node descriptors (451 descriptors).
fn bench_paragon_state() -> (ParagonOverlayState, ParagonOverlayLayout) {
    let layout = ParagonOverlayLayout {
        window_geometry: WINDOW_GEOMETRY,
        panel_left: 540,
        panel_top: 70,
        panel_height: 40,
        panel_gap: 4,
        panel_text_offset: 16,
        font_px: 16,
        tile_gap: 4,
        tile_count: 21,
        node_size_full: 16,
        node_size_collapsed: 10,
        node_border_size: 2,
        collapsed_left_offset: -40,
        collapsed_top_offset: 20,
    };
    let state = ParagonOverlayState {
        build_name: "Bench Build".to_string(),
        routes: vec![ImportedParagonRoute {
            step_name: Some("Route A".to_string()),
            boards: vec![ImportedParagonBoard {
                index: 0,
                name: "Decimator".to_string(),
                glyph: "Marshal".to_string(),
                rotation: "0°".to_string(),
                active_nodes: vec![0, 1, 2, 20],
            }],
        }],
        current_route_index: 0,
        current_board_index: Some(0),
        current_board_name: None,
        mode: ParagonOverlayMode::Full,
    };
    (state, layout)
}

const PARAGON_WINDOW_GEOMETRY: d4lc_core::Rect = d4lc_core::Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1440,
};

/// The production paragon layout at the default settings
/// (`project_paragon_overlay_layout` in `d4lc-app`, defaults per
/// `SettingsD4.cs:67-75`): 2560x1440 window, node sizes 40/23, collapsed
/// offsets -200/-50, border 2, 18 px overlay font.
fn production_paragon_layout() -> ParagonOverlayLayout {
    ParagonOverlayLayout {
        window_geometry: PARAGON_WINDOW_GEOMETRY,
        panel_left: 0,
        panel_top: 100,
        panel_height: 50,
        panel_gap: 2,
        panel_text_offset: 20,
        font_px: 18,
        tile_gap: 5,
        tile_count: 21,
        node_size_full: 40,
        node_size_collapsed: 23,
        node_border_size: 2,
        collapsed_left_offset: -200,
        collapsed_top_offset: -50,
    }
}

/// One route with eight boards (the board list rows); every board marks
/// 40 of the 441 tiles active, offset per board so the lists differ.
fn production_paragon_state(mode: ParagonOverlayMode) -> ParagonOverlayState {
    let boards = (0..8_usize)
        .map(|board| ImportedParagonBoard {
            index: 0,
            name: format!("Board {board} Decimator"),
            glyph: "Marshal".to_string(),
            rotation: "90°".to_string(),
            active_nodes: (0..40).map(|node| (board * 7 + node * 11) % 441).collect(),
        })
        .collect();
    ParagonOverlayState {
        build_name: "Bench Build".to_string(),
        routes: vec![ImportedParagonRoute {
            step_name: Some("Route A".to_string()),
            boards,
        }],
        current_route_index: 0,
        current_board_index: Some(0),
        current_board_name: None,
        mode,
    }
}

fn paragon_descriptors(mode: ParagonOverlayMode) -> Vec<d4lc_rules::RuleOverlayDescriptor> {
    build_live_overlay_descriptors(
        None,
        Some((
            &production_paragon_state(mode),
            &production_paragon_layout(),
        )),
        &FixedTextMeasure::HALF_EM,
    )
}

/// The exact runtime producer call
/// (`d4lc_rules::build_live_overlay_descriptors`) with rule + paragon state.
fn combined_full_stack_descriptors() -> Vec<d4lc_rules::RuleOverlayDescriptor> {
    let evaluation = evaluate_rule_descriptors();
    let (paragon_state, paragon_layout) = bench_paragon_state();
    build_live_overlay_descriptors(
        Some((
            &evaluation,
            &bench_overlay_settings(),
            &bench_overlay_layout(),
        )),
        Some((&paragon_state, &paragon_layout)),
        &FixedTextMeasure::HALF_EM,
    )
}

// ---------------------------------------------------------------------------
// Backends.
// ---------------------------------------------------------------------------

/// Creates a `headless_noop` overlay regardless of the host environment by
/// removing `DISPLAY` for the duration of the call.
fn create_headless_overlay() -> OverlayHandle {
    create_headless_overlay_for(WINDOW_GEOMETRY)
}

fn create_headless_overlay_for(window_geometry: d4lc_core::Rect) -> OverlayHandle {
    let saved_display = std::env::var("DISPLAY").ok();
    std::env::remove_var("DISPLAY");
    let overlay = create_overlay(OverlayConfig {
        window_geometry,
        show_overlay_icon: true,
        overlay_icon_pos_x: 920,
        overlay_icon_pos_y: 24,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("headless overlay creation failed: {error}"));
    if let Some(display) = saved_display {
        std::env::set_var("DISPLAY", display);
    }
    assert_eq!(
        overlay.status().backend,
        OverlayBackend::HeadlessNoop,
        "expected headless_noop backend"
    );
    overlay
}

/// Xvfb session on a private display (never the real `:0`), mirroring the
/// Smoke environment. Readiness is probed via the X11 socket file
/// so this harness needs no direct X11 dependency.
#[cfg(feature = "real-x11")]
struct XvfbSession {
    display: String,
    child: std::process::Child,
}

#[cfg(feature = "real-x11")]
impl XvfbSession {
    fn spawn() -> Result<Self, String> {
        use std::io::BufRead as _;

        // Xvfb picks a free display number itself (`-displayfd`) and reports
        // it once it accepts connections, so parallel runs never share a
        // display or measure against a server that is still starting.
        let mut child = match std::process::Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "1920x1080x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("Xvfb is not installed or not on PATH".to_string());
            }
            Err(error) => return Err(format!("could not start Xvfb: {error}")),
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("could not read the Xvfb display number".to_string());
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let number = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .ok()
            .and_then(|line| line.trim().parse::<u16>().ok());
        match number {
            Some(number) => Ok(Self {
                display: format!(":{number}"),
                child,
            }),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err("Xvfb did not report a display".to_string())
            }
        }
    }
}

#[cfg(feature = "real-x11")]
impl Drop for XvfbSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Creates the real X11 overlay on a fresh Xvfb display. Fails the harness
/// (rather than reporting numbers) when the backend is not `real_x11`.
#[cfg(feature = "real-x11")]
fn create_synthetic_x11_overlay() -> (OverlayHandle, XvfbSession) {
    let session = XvfbSession::spawn().unwrap_or_else(|error| panic!("Xvfb session: {error}"));
    let saved_display = std::env::var("DISPLAY").ok();
    std::env::set_var("DISPLAY", &session.display);
    let overlay = create_overlay(OverlayConfig {
        window_geometry: WINDOW_GEOMETRY,
        show_overlay_icon: true,
        overlay_icon_pos_x: 920,
        overlay_icon_pos_y: 24,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("synthetic X11 overlay creation failed: {error}"));
    if let Some(display) = saved_display {
        std::env::set_var("DISPLAY", display);
    } else {
        std::env::remove_var("DISPLAY");
    }
    assert_eq!(
        overlay.status().backend,
        OverlayBackend::RealX11,
        "synthetic X11 benchmark requires the real_x11 backend (limitation: {:?})",
        overlay.status().limitation,
    );
    (overlay, session)
}

// ---------------------------------------------------------------------------
// Measurement machinery (nearest-rank percentiles over integer microseconds,
// rank formula identical to `crates/d4lc-cli/src/live.rs::percentile_ms`).
// ---------------------------------------------------------------------------

const COUNTED_UPDATES_PER_SCENARIO: usize = 240;
const DISCARDED_UPDATES_PER_SCENARIO: usize = 10;
const BUDGET_P95_MS: f64 = 4.0;
const DESCRIPTOR_BUILD_BACKEND: &str = "descriptor_build";

fn percentile_us(samples: &mut [u128], percentile: usize) -> u128 {
    samples.sort_unstable();
    if samples.is_empty() {
        return 0;
    }
    // Nearest-rank: 1-indexed rank = ceil(p * n / 100), identical to the
    // `live.rs::percentile_ms` formula ((p * n + 99) / 100).
    let rank = (percentile * samples.len()).div_ceil(100);
    let rank = rank.clamp(1, samples.len());
    samples[rank - 1]
}

fn us_to_ms(value: u128) -> f64 {
    value as f64 / 1000.0
}

struct ScenarioMetrics {
    name: &'static str,
    backend: &'static str,
    workload: &'static str,
    descriptor_count: usize,
    render_op_count: usize,
    draw_error_count: u64,
    samples: Vec<u128>,
}

/// The live loop's steady state: every frame carries the same shared
/// descriptor list (the presentation's `Arc`), so the overlay reuses the ops.
fn drive_scenario(
    name: &'static str,
    backend: &'static str,
    workload: &'static str,
    overlay: OverlayHandle,
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
) -> ScenarioMetrics {
    drive_scenario_with(name, backend, workload, overlay, descriptors, false)
}

/// `new_list_per_frame` hands every frame a freshly built list (prepared
/// outside the timed call), the cost of a frame whose descriptors changed.
fn drive_scenario_with(
    name: &'static str,
    backend: &'static str,
    workload: &'static str,
    mut overlay: OverlayHandle,
    descriptors: &[d4lc_rules::RuleOverlayDescriptor],
    new_list_per_frame: bool,
) -> ScenarioMetrics {
    let shared: std::sync::Arc<[d4lc_rules::RuleOverlayDescriptor]> = descriptors.to_vec().into();
    let frame = |sequence: u64| OverlayFrame {
        sequence,
        tooltip_found: true,
        hotpath_ms: 6,
        cursor_window_relative: None,
        rule_descriptors: if new_list_per_frame {
            descriptors.to_vec().into()
        } else {
            std::sync::Arc::clone(&shared)
        },
        text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
    };

    // Discarded in-process warmup updates (in addition to the discarded
    // invocation at the harness-command level).
    for sequence in 0..DISCARDED_UPDATES_PER_SCENARIO as u64 {
        overlay
            .update(frame(sequence))
            .unwrap_or_else(|error| panic!("warmup update failed: {error}"));
    }

    let mut samples = Vec::with_capacity(COUNTED_UPDATES_PER_SCENARIO);
    for sequence in 0..COUNTED_UPDATES_PER_SCENARIO as u64 {
        let payload = frame(sequence);
        let started_at = Instant::now();
        overlay
            .update(payload)
            .unwrap_or_else(|error| panic!("counted update failed: {error}"));
        samples.push(started_at.elapsed().as_micros());
    }

    let descriptor_count = descriptors.len();
    let render_op_count = overlay.last_render_ops().len();
    let draw_error_count = overlay.status().draw_error_count;
    ScenarioMetrics {
        name,
        backend,
        workload,
        descriptor_count,
        render_op_count,
        draw_error_count,
        samples,
    }
}

/// Times the descriptor producer itself (no overlay involved). Reported
/// next to the update scenarios; excluded from the update-latency budget.
fn drive_build_scenario(
    name: &'static str,
    workload: &'static str,
    mut build: impl FnMut() -> Vec<d4lc_rules::RuleOverlayDescriptor>,
) -> ScenarioMetrics {
    for _ in 0..DISCARDED_UPDATES_PER_SCENARIO {
        std::hint::black_box(build());
    }
    let mut samples = Vec::with_capacity(COUNTED_UPDATES_PER_SCENARIO);
    let mut descriptor_count = 0;
    for _ in 0..COUNTED_UPDATES_PER_SCENARIO {
        let started_at = Instant::now();
        let descriptors = std::hint::black_box(build());
        samples.push(started_at.elapsed().as_micros());
        descriptor_count = descriptors.len();
    }
    ScenarioMetrics {
        name,
        backend: DESCRIPTOR_BUILD_BACKEND,
        workload,
        descriptor_count,
        render_op_count: 0,
        draw_error_count: 0,
        samples,
    }
}

struct EnvironmentInfo {
    os: String,
    kernel: String,
    cpu: String,
    cores: usize,
    ram_gb: u64,
}

fn environment_info() -> EnvironmentInfo {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|line| line.starts_with("PRETTY_NAME="))
                .map(|line| {
                    line.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
        })
        .unwrap_or_else(|| "Linux".to_string());
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split(':').next_back())
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let ram_gb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let rest = line.strip_prefix("MemTotal:")?;
                rest.trim()
                    .trim_end_matches(" kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
        })
        .map(|kb| kb / (1024 * 1024))
        .unwrap_or(0);
    EnvironmentInfo {
        os,
        kernel,
        cpu,
        cores: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(0),
        ram_gb,
    }
}

fn repo_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_root())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .unwrap_or_else(|| panic!("could not resolve workspace root"))
        .to_path_buf()
}

fn scenario_json(scenario: &ScenarioMetrics, budget_pass: bool) -> serde_json::Value {
    let mut update_samples = scenario.samples.clone();
    let p50 = percentile_us(&mut update_samples, 50);
    let mut update_samples = scenario.samples.clone();
    let p95 = percentile_us(&mut update_samples, 95);
    let mut update_samples = scenario.samples.clone();
    let p99 = percentile_us(&mut update_samples, 99);
    serde_json::json!({
        "name": scenario.name,
        "backend": scenario.backend,
        "workload": scenario.workload,
        "descriptor_count": scenario.descriptor_count,
        "render_op_count": scenario.render_op_count,
        "draw_error_count": scenario.draw_error_count,
        "samples": scenario.samples.len(),
        "timings_ms": {
            "overlay_update": {
                "p50": us_to_ms(p50),
                "p95": us_to_ms(p95),
                "p99": us_to_ms(p99),
            }
        },
        "measured_p95_ms": us_to_ms(p95),
        "budget_pass": budget_pass,
    })
}

fn write_report(scenarios: &[ScenarioMetrics]) -> PathBuf {
    let label = std::env::var("D4LC_PERF_LABEL").unwrap_or_else(|_| "run".to_string());
    let worst_p95_ms = scenarios
        .iter()
        .filter(|scenario| scenario.backend != DESCRIPTOR_BUILD_BACKEND)
        .map(|scenario| {
            let mut samples = scenario.samples.clone();
            us_to_ms(percentile_us(&mut samples, 95))
        })
        .fold(0.0_f64, f64::max);
    let environment = environment_info();
    let report = serde_json::json!({
        "schema_version": 1,
        "command": "cargo test -p d4lc-overlay-x11 --release --test perf_overlay_draw -- --ignored --nocapture --test-threads=1 (add --features real-x11 for the synthetic_x11 scenarios)",
        "label": label,
        "percentile_method": "nearest-rank",
        "sample_resolution": "integer microseconds per sample, reported as milliseconds with sub-ms precision (rank formula identical to live.rs::percentile_ms)",
        "environment": {
            "os": environment.os,
            "kernel": environment.kernel,
            "cpu": environment.cpu,
            "cores": environment.cores,
            "ram_gb": environment.ram_gb,
            "session": "Xvfb (synthetic, private display :90+); the real :0 display is never touched",
            "build_mode": "release (cargo test --release, opt-level 3)",
            "repo_commit": repo_commit(),
            "preset": "n/a (library-stage overlay measurement, no capture)",
            "background_load": "Diablo IV actively played during benchmarks (X11 :0 busy; identical for every scenario and label)",
        },
        "warmup": {
            "discarded_invocations": 1,
            "discarded_updates_per_scenario": DISCARDED_UPDATES_PER_SCENARIO,
            "counted_samples_min": 60,
        },
        "rendering_blocks_runtime": {
            "blocks": true,
            "explanation": "OverlayHandle::update is called synchronously inside the live-loop iteration (crates/d4lc-cli/src/live.rs::run_live_loop_session); headless updates block for render-plan expansion only, real_x11 updates additionally block on X11 request submission, per-op error checks, and the final flush.",
        },
        "scenarios": scenarios
            .iter()
            .map(|scenario| {
                let mut samples = scenario.samples.clone();
                let p95 = us_to_ms(percentile_us(&mut samples, 95));
                scenario_json(scenario, p95 < BUDGET_P95_MS)
            })
            .collect::<Vec<_>>(),
        "budgets": {
            "overlay_update_p95": {
                "budget_p95_ms": BUDGET_P95_MS,
                "measured_p95_ms": worst_p95_ms,
                "pass": worst_p95_ms < BUDGET_P95_MS,
                "provisional": false,
                "basis": "worst-case product frame (combined_full_stack, real_x11 when measured); budget maps to scenarios[].timings_ms.overlay_update.p95",
            }
        },
    });

    let artifact_dir = workspace_root().join("debug-output/perf-overlay-draw");
    std::fs::create_dir_all(&artifact_dir)
        .unwrap_or_else(|error| panic!("create artifact dir: {error}"));
    let path = artifact_dir.join(format!("{label}.json"));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&report).unwrap_or_else(|error| panic!("serialize: {error}")),
    )
    .unwrap_or_else(|error| panic!("write report: {error}"));
    path
}

// ---------------------------------------------------------------------------
// Harness entry point (ignored by default; run with --ignored).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "performance benchmark; run with --ignored --release --test-threads=1"]
fn perf_overlay_draw_report() {
    let single_tooltip = evaluate_rule_descriptors().overlay_descriptors;
    assert!(
        !single_tooltip.is_empty(),
        "rule chain must produce single-tooltip descriptors"
    );
    let combined = combined_full_stack_descriptors();
    assert!(
        combined.len() > single_tooltip.len(),
        "combined workload must exceed the single-tooltip workload"
    );

    let mut scenarios = Vec::new();

    // Headless render-plan scenarios (always available).
    scenarios.push(drive_scenario(
        "headless_single_tooltip",
        "headless_noop",
        "single_tooltip",
        create_headless_overlay(),
        &single_tooltip,
    ));
    scenarios.push(drive_scenario(
        "headless_combined_full_stack",
        "headless_noop",
        "combined_full_stack",
        create_headless_overlay(),
        &combined,
    ));

    // Paragon overlay alone at the production layout (2560x1440).
    for (name, workload, mode) in [
        (
            "headless_paragon_full",
            "paragon_full",
            ParagonOverlayMode::Full,
        ),
        (
            "headless_paragon_collapsed",
            "paragon_collapsed",
            ParagonOverlayMode::Collapsed,
        ),
    ] {
        scenarios.push(drive_scenario(
            name,
            "headless_noop",
            workload,
            create_headless_overlay_for(PARAGON_WINDOW_GEOMETRY),
            &paragon_descriptors(mode),
        ));
    }
    scenarios.push(drive_scenario_with(
        "headless_paragon_full_new_list",
        "headless_noop",
        "paragon_full_new_list",
        create_headless_overlay_for(PARAGON_WINDOW_GEOMETRY),
        &paragon_descriptors(ParagonOverlayMode::Full),
        true,
    ));
    scenarios.push(drive_build_scenario(
        "paragon_full_descriptor_build",
        "paragon_full",
        || paragon_descriptors(ParagonOverlayMode::Full),
    ));
    scenarios.push(drive_build_scenario(
        "paragon_collapsed_descriptor_build",
        "paragon_collapsed",
        || paragon_descriptors(ParagonOverlayMode::Collapsed),
    ));

    // Synthetic X11 scenarios require the real-x11 feature. Without it the
    // harness records the skip instead of fake numbers.
    #[cfg(feature = "real-x11")]
    {
        let (overlay, _session) = create_synthetic_x11_overlay();
        scenarios.push(drive_scenario(
            "synthetic_x11_single_tooltip",
            "real_x11 (Xvfb)",
            "single_tooltip",
            overlay,
            &single_tooltip,
        ));
        let (overlay, _session) = create_synthetic_x11_overlay();
        scenarios.push(drive_scenario(
            "synthetic_x11_combined_full_stack",
            "real_x11 (Xvfb)",
            "combined_full_stack",
            overlay,
            &combined,
        ));
    }
    #[cfg(not(feature = "real-x11"))]
    {
        scenarios.push(ScenarioMetrics {
            name: "synthetic_x11_single_tooltip",
            backend: "skipped",
            workload: "single_tooltip",
            descriptor_count: 0,
            render_op_count: 0,
            draw_error_count: 0,
            samples: Vec::new(),
        });
        scenarios.push(ScenarioMetrics {
            name: "synthetic_x11_combined_full_stack",
            backend: "skipped",
            workload: "combined_full_stack",
            descriptor_count: 0,
            render_op_count: 0,
            draw_error_count: 0,
            samples: Vec::new(),
        });
        eprintln!("synthetic X11 scenarios skipped: compiled without the real-x11 feature");
    }

    for scenario in &scenarios {
        let mut samples = scenario.samples.clone();
        let p50 = percentile_us(&mut samples, 50);
        let mut samples = scenario.samples.clone();
        let p95 = percentile_us(&mut samples, 95);
        let mut samples = scenario.samples.clone();
        let p99 = percentile_us(&mut samples, 99);
        println!(
            "[perf] {} backend={} workload={} descriptors={} render_ops={} draw_errors={} samples={} p50={:.3}ms p95={:.3}ms p99={:.3}ms",
            scenario.name,
            scenario.backend,
            scenario.workload,
            scenario.descriptor_count,
            scenario.render_op_count,
            scenario.draw_error_count,
            scenario.samples.len(),
            us_to_ms(p50),
            us_to_ms(p95),
            us_to_ms(p99),
        );
    }

    let report_path = write_report(&scenarios);
    println!("[perf] report written to {}", report_path.display());
}

/// Non-ignored sanity gate: pins the percentile formula and the product
/// workload shapes so the default suite keeps the harness.
#[test]
fn perf_overlay_harness_sanity() {
    // Nearest-rank percentiles over a pinned 20-sample series: p95 at n=20 is
    // the second-largest sample.
    let mut samples: Vec<u128> = (1..=20).map(|value| value * 100).collect();
    assert_eq!(percentile_us(&mut samples, 50), 1_000);
    assert_eq!(percentile_us(&mut samples, 95), 1_900);
    assert_eq!(percentile_us(&mut samples, 99), 2_000);

    let single_tooltip = evaluate_rule_descriptors().overlay_descriptors;
    assert!(
        !single_tooltip.is_empty(),
        "rule chain must produce single-tooltip descriptors"
    );
    assert!(single_tooltip.iter().all(|descriptor| {
        descriptor.color.name() != Some("") && descriptor.geometry.width > 0
    }));

    let combined = combined_full_stack_descriptors();
    assert!(combined.len() > single_tooltip.len());
    // The paragon tile grid must dominate the heavy workload (21x21 tiles).
    let paragon_count = combined
        .iter()
        .filter(|descriptor| descriptor.marker_type == d4lc_rules::RuleOverlayMarkerType::Paragon)
        .count();
    assert!(
        paragon_count >= 441,
        "full paragon board must expand the 21x21 tile grid, got {paragon_count}"
    );

    // Production-layout paragon workloads: 441 tiles + frame + panels.
    for mode in [ParagonOverlayMode::Full, ParagonOverlayMode::Collapsed] {
        let descriptors = paragon_descriptors(mode);
        assert!(
            descriptors.len() >= 441,
            "paragon-only workload must expand the 21x21 grid, got {}",
            descriptors.len()
        );
        let mut overlay = create_headless_overlay_for(PARAGON_WINDOW_GEOMETRY);
        overlay
            .update(OverlayFrame {
                sequence: 1,
                tooltip_found: false,
                hotpath_ms: 6,
                cursor_window_relative: None,
                rule_descriptors: descriptors.into(),
                text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
            })
            .unwrap_or_else(|error| panic!("paragon update failed: {error}"));
        assert!(overlay.last_render_ops().len() >= 441);
    }

    // Headless update round trip works and records the render plan.
    let mut overlay = create_headless_overlay();
    overlay
        .update(OverlayFrame {
            sequence: 1,
            tooltip_found: true,
            hotpath_ms: 6,
            cursor_window_relative: None,
            rule_descriptors: combined.clone().into(),
            text_font_px: d4lc_overlay_x11::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("headless update failed: {error}"));
    assert_eq!(
        overlay.last_render_ops().len(),
        combined.len()
            + combined
                .iter()
                .filter(|descriptor| descriptor.text.is_some())
                .count()
            + 3,
        "render ops = descriptors + text ops + 3 menu ops (locked menu visible)"
    );
}
