//! Release-mode latency harness for the parse + rule evaluation hot path.
//!
//! Measures per-tooltip OCR-result latency of
//! `parse_item_tooltip_from_ocr_segments_with_fuzzy` followed by the
//! `d4lc-rules` evaluation chain (multi-build presets, item power/minimal
//! value filters, non-affix matching, trade lookup, overlay descriptors)
//! over a fixture set covering item type/power, affixes, aspects, uniques,
//! runes, sigils, multi-build rules, trade matches, and no-match cases.
//!
//! Methodology: one discarded warmup invocation, >= 60 counted samples per
//! scenario and metric (16 iterations x 10 fixtures = 160), nearest-rank
//! percentiles over integer-microsecond samples (the `live.rs::percentile_ms`
//! rank formula; sub-millisecond precision is kept because the budget is
//! 5 ms per tooltip), and an allocation-counting global allocator for the
//! allocation hotspot metrics.
//!
//! Run (bench only, not part of the default suite):
//! ```text
//! cargo test -p d4lc-parse --release --test perf_parse_rules_latency \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//! The report label defaults to `run`; set `D4LC_PERF_LABEL=before|after`
//! for A/B evidence runs. Reports are written to
//! `debug-output/perf-parse-rules/<label>.json` under the workspace root.

use d4lc_assets::AssetRepository;
use d4lc_fuzzy::FuzzyIndexes;
use d4lc_parse::{
    convert_to_item_power, convert_to_item_type, parse_item_tooltip_from_ocr_segments_with_fuzzy,
    ItemTooltipAffix, ItemTooltipParseResult, TooltipOcrLineInput, TooltipOcrSegmentInput,
    TooltipOcrSegmentKind,
};
use d4lc_rules::{
    apply_item_power_and_value_filters, apply_overlay_descriptors, apply_trade_item_lookup,
    evaluate_multi_build_affix_presets, evaluate_non_affix_multi_build, AffixPreset,
    AffixRequirement, AffixValueThreshold, MultiBuildEntry, MultiBuildSettings, NonAffixPreset,
    NonAffixRequirement, ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules,
    ParsedNonAffixForRules, RuleFilterSettings, RuleNonAffixKind, RuleOverlayLayout,
    RuleOverlayMarkerMode, RuleOverlaySettings, TradeAffixRequirement, TradeItemRule,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Allocation-counting global allocator (allocation-hotspot metric).
// `unsafe_code` is workspace-denied; a `GlobalAlloc` implementation cannot be
// written without it, so this module is the only sanctioned exception.
// ---------------------------------------------------------------------------
#[allow(unsafe_code)]
mod alloc_tracking {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    pub struct AllocStats {
        pub alloc_count: AtomicU64,
        pub dealloc_count: AtomicU64,
        pub alloc_bytes: AtomicU64,
        pub live_bytes: AtomicI64,
        pub max_live_bytes: AtomicUsize,
    }

    impl AllocStats {
        pub fn snapshot(&self) -> AllocSnapshot {
            AllocSnapshot {
                alloc_count: self.alloc_count.load(Ordering::Relaxed),
                dealloc_count: self.dealloc_count.load(Ordering::Relaxed),
                alloc_bytes: self.alloc_bytes.load(Ordering::Relaxed),
                max_live_bytes: self.max_live_bytes.load(Ordering::Relaxed),
            }
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    pub struct AllocSnapshot {
        pub alloc_count: u64,
        pub dealloc_count: u64,
        pub alloc_bytes: u64,
        pub max_live_bytes: usize,
    }

    impl AllocSnapshot {
        pub fn delta_since(&self, earlier: AllocSnapshot) -> AllocSnapshot {
            AllocSnapshot {
                alloc_count: self.alloc_count - earlier.alloc_count,
                dealloc_count: self.dealloc_count - earlier.dealloc_count,
                alloc_bytes: self.alloc_bytes - earlier.alloc_bytes,
                // max_live is a high-water mark; a per-window max is not
                // reconstructible from snapshots, so the delta reports 0.
                max_live_bytes: 0,
            }
        }
    }

    pub static STATS: AllocStats = AllocStats {
        alloc_count: AtomicU64::new(0),
        dealloc_count: AtomicU64::new(0),
        alloc_bytes: AtomicU64::new(0),
        live_bytes: AtomicI64::new(0),
        max_live_bytes: AtomicUsize::new(0),
    };

    pub struct TrackingAllocator;

    // SAFETY: every call forwards the caller's `layout` / `pointer` unchanged
    // to `System`, so `System`'s `GlobalAlloc` contract holds; the counters
    // are lock-free atomics and never allocate.
    #[allow(unsafe_code)]
    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = System.alloc(layout);
            if !pointer.is_null() {
                STATS.alloc_count.fetch_add(1, Ordering::Relaxed);
                STATS
                    .alloc_bytes
                    .fetch_add(layout.size() as u64, Ordering::Relaxed);
                let live = STATS
                    .live_bytes
                    .fetch_add(layout.size() as i64, Ordering::Relaxed)
                    + layout.size() as i64;
                STATS
                    .max_live_bytes
                    .fetch_max(live.max(0) as usize, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            STATS.dealloc_count.fetch_add(1, Ordering::Relaxed);
            STATS
                .live_bytes
                .fetch_sub(layout.size() as i64, Ordering::Relaxed);
            System.dealloc(pointer, layout)
        }
    }

    #[global_allocator]
    static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;
}

use alloc_tracking::STATS;

// ---------------------------------------------------------------------------
// Fixture set. OCR text mirrors the existing parser/rule test fixtures so the
// measured workload is the same one the parity tests assert on.
// ---------------------------------------------------------------------------

struct TooltipFixture {
    name: &'static str,
    category: &'static str,
    segments: Vec<TooltipOcrSegmentInput>,
}

fn line_input(text: &str) -> TooltipOcrLineInput {
    TooltipOcrLineInput {
        text: text.to_string(),
        rect: None,
    }
}

fn segment(
    index: usize,
    kind: TooltipOcrSegmentKind,
    lines: &[&str],
    converted_text: &str,
) -> TooltipOcrSegmentInput {
    TooltipOcrSegmentInput {
        index,
        kind,
        raw_text: lines.join("\n"),
        converted_text: converted_text.to_string(),
        lines: lines.iter().map(|line| line_input(line)).collect(),
        marker_name: None,
    }
}

fn affix_segment(index: usize, raw_text: &str, converted_text: &str) -> TooltipOcrSegmentInput {
    segment(
        index,
        TooltipOcrSegmentKind::Affix,
        &[raw_text],
        converted_text,
    )
}

fn affix_segment_with_marker(
    index: usize,
    raw_text: &str,
    converted_text: &str,
    marker_name: &str,
) -> TooltipOcrSegmentInput {
    let mut segment = affix_segment(index, raw_text, converted_text);
    segment.marker_name = Some(marker_name.to_string());
    segment
}

fn fixtures() -> Vec<TooltipFixture> {
    vec![
        // Item type + item power + four affixes + metadata.
        TooltipFixture {
            name: "legendary_amulet_four_affixes",
            category: "item_affixes",
            segments: vec![
                segment(
                    0,
                    TooltipOcrSegmentKind::UpperTooltip,
                    &["Flickersteel", "Ancestral Legendary Amulet", "925 Item Power"],
                    "",
                ),
                affix_segment(1, "+32.0% Attack Speed", "Attack Speed"),
                affix_segment(2, "+715 Thorns", "Thorns"),
                affix_segment(3, "+58.0% Critical Strike Damage", "Critical Strike Damage"),
                affix_segment(4, "+[150 - 200] Life On Kill", "Life On Kill"),
                segment(5, TooltipOcrSegmentKind::Generic, &["Requires Level 70", "Account Bound", "Sell Value: 1234"], ""),
            ],
        },
        // Weapon details + aspect legendary power.
        TooltipFixture {
            name: "legendary_weapon_with_aspect",
            category: "aspect",
            segments: vec![
                segment(
                    0,
                    TooltipOcrSegmentKind::UpperTooltip,
                    &[
                        "HELLBENT COMMANDER DEMONBLADE",
                        "Ancestral Legendary Two- } Handed Sword",
                        "900 Item Power",
                    ],
                    "",
                ),
                segment(1, TooltipOcrSegmentKind::Generic, &["1234 Damage Per Second", "987 - 1234 Damage Per Hit", "1.15 Attacks per Second"], ""),
                affix_segment(2, "+22.5% Attack Speed", "Attack Speed"),
                segment(
                    3,
                    TooltipOcrSegmentKind::Aspect,
                    &["Critical Strikes with Core Skills increase your Attack Speed by for seconds."],
                    "Critical Strikes with Core Skills increase your Attack Speed by for seconds.",
                ),
            ],
        },
        // Unique aspect + socket.
        TooltipFixture {
            name: "unique_boots_with_socket",
            category: "unique",
            segments: vec![
                segment(
                    0,
                    TooltipOcrSegmentKind::UpperTooltip,
                    &["Echo of Harlequin Crest", "Unique Boots", "870 Item Power"],
                    "",
                ),
                segment(
                    1,
                    TooltipOcrSegmentKind::UniqueAspect,
                    &["Ground Stomp's damage is increased by plus an additional amount based on the distance traveled since the last casted Ground Stomp's, up to."],
                    "Ground Stomp's damage is increased by plus an additional amount based on the distance traveled since the last casted Ground Stomp's, up to.",
                ),
                affix_segment(2, "+16.5% Movement Speed", "Movement Speed"),
                segment(3, TooltipOcrSegmentKind::Socket, &["Empty Socket"], "Empty Socket"),
            ],
        },
        // Rune segment.
        TooltipFixture {
            name: "rune_item",
            category: "rune",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::UpperTooltip, &["Yul Rune"], ""),
                segment(
                    1,
                    TooltipOcrSegmentKind::Rune,
                    &["Travel meters.", "Cooldown text", "ignored"],
                    "Travel meters.\nCooldown text\nignored",
                ),
            ],
        },
        // Sigil segment.
        TooltipFixture {
            name: "nightmare_sigil",
            category: "sigil",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::UpperTooltip, &["Nightmare Sigil"], ""),
                segment(
                    1,
                    TooltipOcrSegmentKind::Sigil,
                    &["Abandoned Mineworks in Kehjistan\nTier 100"],
                    "Abandoned Mineworks in Kehjistan\nTier 100",
                ),
            ],
        },
        // No-match candidates: junk affix lines (full fuzzy scan, best score
        // reported with the upstream zero affix threshold) plus a junk rune
        // line whose lookup is rejected by the 75 item-type/rune threshold.
        TooltipFixture {
            name: "no_match_affixes",
            category: "no_match",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::UpperTooltip, &["Ancestral Legendary Ring", "640 Item Power"], ""),
                affix_segment(1, "+98765 Zzzqqq Wxxvvv", "Zzzqqq Wxxvvv"),
                affix_segment(2, "+98765 Qqzzvv Plmkkk", "Qqzzvv Plmkkk"),
                segment(
                    3,
                    TooltipOcrSegmentKind::Rune,
                    &["Xzz Yqq Vww Ccb", "Junk rune text", "ignored"],
                    "Xzz Yqq Vww Ccb\nJunk rune text\nignored",
                ),
            ],
        },
        // Magic rarity, fewer affixes.
        TooltipFixture {
            name: "magic_ring_two_affixes",
            category: "item_affixes",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::UpperTooltip, &["Sparkshard Loop", "Magic Ring", "580 Item Power"], ""),
                affix_segment(1, "+15.0% Attack Speed", "Attack Speed"),
                affix_segment(2, "+[125 - 150] Armor", "Armor"),
            ],
        },
        // UI chrome, comparison mode, and ignored lines (no fuzzy work).
        TooltipFixture {
            name: "ui_chrome_and_comparison",
            category: "no_match",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::Generic, &["Scroll Down", "Mark as Junk", "Left Mouse Button"], ""),
                segment(1, TooltipOcrSegmentKind::Generic, &["Properties Lost When Equipped", "Requires Level 60"], ""),
                segment(2, TooltipOcrSegmentKind::Generic, &["+105.0% Armor"], ""),
            ],
        },
        // Existing weapon OCR fixture text (known parser decision point).
        TooltipFixture {
            name: "ancestral_two_handed_weapon",
            category: "item_affixes",
            segments: vec![
                segment(
                    0,
                    TooltipOcrSegmentKind::UpperTooltip,
                    &["HELLBENT", "COMMANDER", "DEMONBLADE", "Ancestral Legendary Two- }", "Handed Sword", "900 Item Power", "25 (% +25) Quality"],
                    "",
                ),
                affix_segment(1, "+26.0% Damage", "Damage"),
                affix_segment(2, "+34.5% Vulnerable Damage", "Vulnerable Damage"),
            ],
        },
        // Greater / implicit / tempered marker paths.
        TooltipFixture {
            name: "rare_gloves_marked_affixes",
            category: "item_affixes",
            segments: vec![
                segment(0, TooltipOcrSegmentKind::UpperTooltip, &["Grasps of Ruin", "Ancestral Rare Gloves", "910 Item Power"], ""),
                affix_segment_with_marker(1, "+44.0% Attack Speed", "Attack Speed", "Dot Affixes Greater"),
                affix_segment(2, "+12.0% Implicit Resistance", "Implicit Resistance"),
                affix_segment_with_marker(3, "+18.0% Tempered Close Damage", "Tempered Close Damage", "Tempering"),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Parse -> rules glue (the consumer-side conversion the rules API expects).
// ---------------------------------------------------------------------------

fn affix_kind(affix: &ItemTooltipAffix) -> ParsedAffixKind {
    if affix.is_implicit {
        ParsedAffixKind::Implicit
    } else if affix.is_tempered {
        ParsedAffixKind::Tempered
    } else if affix.is_greater_affix {
        ParsedAffixKind::Greater
    } else {
        ParsedAffixKind::Normal
    }
}

fn item_for_rules(result: &ItemTooltipParseResult) -> ParsedItemForRules {
    let item_type = if !result.sigils.is_empty() {
        "sigil".to_string()
    } else if !result.runes.is_empty() {
        "rune".to_string()
    } else {
        result
            .base_type
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    };
    let affixes = result
        .affixes
        .iter()
        .map(|affix| ParsedAffixForRules {
            affix_id: affix.affix_id.clone().unwrap_or_default(),
            label: affix.affix_label.clone(),
            value_text: affix.value_text.clone(),
            source_text: affix.raw_text.clone(),
            row_id: Some(affix.row_id),
            kind: affix_kind(affix),
        })
        .collect();
    ParsedItemForRules {
        item_type,
        rarity: result.rarity.map(|rarity| rarity.as_str().to_string()),
        affixes,
    }
}

fn non_affix_for_rules(result: &ItemTooltipParseResult) -> Vec<ParsedNonAffixForRules> {
    let mut parsed = Vec::new();
    if let Some(power) = &result.legendary_power {
        if let Some(power_id) = &power.power_id {
            parsed.push(ParsedNonAffixForRules {
                id: power_id.clone(),
                item_type: result
                    .base_type
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                kind: if power.is_unique {
                    RuleNonAffixKind::Unique
                } else {
                    RuleNonAffixKind::Aspect
                },
                is_unique_item: power.is_unique,
            });
        }
    }
    for rune in &result.runes {
        if let Some(rune_id) = &rune.rune_id {
            parsed.push(ParsedNonAffixForRules {
                id: rune_id.clone(),
                item_type: "rune".to_string(),
                kind: RuleNonAffixKind::Rune,
                is_unique_item: false,
            });
        }
    }
    for sigil in &result.sigils {
        if let Some(sigil_id) = &sigil.sigil_id {
            parsed.push(ParsedNonAffixForRules {
                id: sigil_id.clone(),
                item_type: "sigil".to_string(),
                kind: RuleNonAffixKind::Sigil,
                is_unique_item: false,
            });
        }
    }
    parsed
}

/// Rule configuration derived from the actual parsed fixture ids so the
/// matched branches (wanted decision, trade match, non-affix match) are
/// exercised with real canonical ids from the loaded data.
struct RuleBenchmarkConfig {
    presets: Vec<AffixPreset>,
    non_affix_presets: Vec<NonAffixPreset>,
    multi_build: MultiBuildSettings,
    filters: RuleFilterSettings,
    affix_thresholds: Vec<AffixValueThreshold>,
    trade_items: Vec<TradeItemRule>,
    overlay_settings: RuleOverlaySettings,
    overlay_layout: RuleOverlayLayout,
}

fn build_rule_config(
    fixture_results: &[(&'static str, ItemTooltipParseResult)],
) -> RuleBenchmarkConfig {
    let amulet = &fixture_results[0].1;
    let amulet_item_type = amulet.base_type.clone().unwrap_or_default();
    let required_ids: Vec<String> = amulet
        .affixes
        .iter()
        .filter_map(|affix| affix.affix_id.clone())
        .take(3)
        .collect();
    assert!(
        !required_ids.is_empty(),
        "fixture amulet must produce at least one accepted affix id for the rule config"
    );

    let presets = vec![AffixPreset {
        name: "Bench".to_string(),
        affixes: required_ids
            .iter()
            .map(|affix_id| AffixRequirement {
                affix_id: affix_id.clone(),
                label: None,
                item_type: amulet_item_type.clone(),
                color: "Green".to_string(),
                is_any_type: false,
                is_greater: false,
                is_implicit: false,
                is_tempered: false,
                tuning_prisms: Vec::new(),
            })
            .collect(),
    }];

    let mut powers = Vec::new();
    let mut runes = Vec::new();
    let mut sigils = Vec::new();
    for (_, result) in fixture_results {
        for parsed in non_affix_for_rules(result) {
            match parsed.kind {
                RuleNonAffixKind::Aspect | RuleNonAffixKind::Unique => {
                    powers.push(NonAffixRequirement {
                        id: parsed.id,
                        item_type: None,
                        color: "Gold".to_string(),
                        kind: parsed.kind,
                    });
                }
                RuleNonAffixKind::Rune => runes.push(NonAffixRequirement {
                    id: parsed.id,
                    item_type: None,
                    color: "Purple".to_string(),
                    kind: RuleNonAffixKind::Rune,
                }),
                RuleNonAffixKind::Sigil => sigils.push(NonAffixRequirement {
                    id: parsed.id,
                    item_type: Some("sigil".to_string()),
                    color: "Teal".to_string(),
                    kind: RuleNonAffixKind::Sigil,
                }),
            }
        }
    }
    let non_affix_presets = vec![NonAffixPreset {
        name: "Bench".to_string(),
        powers,
        runes,
        sigils,
    }];

    let affix_thresholds = required_ids
        .first()
        .map(|affix_id| AffixValueThreshold {
            affix_id: affix_id.clone(),
            threshold: 20.0,
        })
        .into_iter()
        .collect();

    let trade_items = vec![
        TradeItemRule {
            trade_item_id: "trade-belt-no-match".to_string(),
            item_type: "Belt".to_string(),
            value_text: "1b".to_string(),
            affixes: vec![TradeAffixRequirement {
                affix_id: required_ids[0].clone(),
                is_greater: false,
            }],
        },
        TradeItemRule {
            trade_item_id: "trade-amulet-match".to_string(),
            item_type: amulet_item_type.clone(),
            value_text: "5b".to_string(),
            affixes: vec![TradeAffixRequirement {
                affix_id: required_ids[0].clone(),
                is_greater: false,
            }],
        },
        TradeItemRule {
            trade_item_id: "trade-greater-required".to_string(),
            item_type: amulet_item_type.clone(),
            value_text: "9b".to_string(),
            affixes: vec![TradeAffixRequirement {
                affix_id: required_ids[0].clone(),
                is_greater: true,
            }],
        },
    ];

    let affix_locations = amulet
        .affixes
        .iter()
        .map(|affix| d4lc_rules::RuleAffixOverlayLocation {
            row_id: Some(affix.row_id),
            geometry: d4lc_core::Rect {
                x: 40,
                y: 200,
                width: 300,
                height: 20,
            },
            text: None,
        })
        .collect();

    RuleBenchmarkConfig {
        presets,
        multi_build: MultiBuildSettings {
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
        },
        filters: RuleFilterSettings {
            is_item_power_limit_enabled: true,
            item_power_limit: 800,
            is_minimal_affix_value_filter_enabled: true,
        },
        affix_thresholds,
        trade_items,
        overlay_settings: RuleOverlaySettings {
            marker_mode: RuleOverlayMarkerMode::ShowAll,
            is_minimal_affix_value_filter_enabled: true,
            is_trade_overlay_enabled: true,
            item_power_limit_check_ok: true,
            is_tuning_prism_hints_enabled: true,
        },
        overlay_layout: RuleOverlayLayout {
            tooltip_rect: d4lc_core::Rect {
                x: 0,
                y: 0,
                width: 500,
                height: 700,
            },
            affix_locations,
            power_location: None,
            power_locations: Vec::new(),
            rune_locations: Vec::new(),
            sigil_location: None,
            sigil_locations: Vec::new(),
        },
        non_affix_presets,
    }
}

/// The full per-tooltip rule evaluation chain (the live consumer sequence).
fn evaluate_rules(
    result: &ItemTooltipParseResult,
    config: &RuleBenchmarkConfig,
) -> d4lc_rules::RuleEvaluationResult {
    let item = item_for_rules(result);
    let mut evaluation =
        evaluate_multi_build_affix_presets(&item, &config.multi_build, &config.presets);
    apply_item_power_and_value_filters(
        &mut evaluation,
        result.item_power,
        item.item_type.as_str(),
        &config.filters,
        &config.affix_thresholds,
    );
    let parsed_non_affix = non_affix_for_rules(result);
    let _non_affix_matches = evaluate_non_affix_multi_build(
        &parsed_non_affix,
        &config.multi_build,
        &config.non_affix_presets,
    );
    apply_trade_item_lookup(&mut evaluation, &item, &config.trade_items);
    apply_overlay_descriptors(
        &mut evaluation,
        &config.overlay_settings,
        &config.overlay_layout,
    );
    evaluation
}

// ---------------------------------------------------------------------------
// Measurement machinery.
// ---------------------------------------------------------------------------

const ITERATIONS: usize = 16;
const BUDGET_P95_MS: f64 = 5.0;

#[derive(Debug, Default)]
struct ScenarioMetrics {
    parse_us: Vec<u128>,
    rules_us: Vec<u128>,
    total_us: Vec<u128>,
    parse_allocs: u64,
    rules_allocs: u64,
    parse_alloc_bytes: u64,
    rules_alloc_bytes: u64,
    fuzzy_lookups: u64,
    fuzzy_accepted: u64,
    fuzzy_rejected: u64,
    fuzzy_candidate_sum: u64,
    wanted_decisions: u64,
    unwanted_decisions: u64,
    trade_matches: u64,
}

impl ScenarioMetrics {
    fn new(capacity: usize) -> Self {
        Self {
            parse_us: Vec::with_capacity(capacity),
            rules_us: Vec::with_capacity(capacity),
            total_us: Vec::with_capacity(capacity),
            ..Self::default()
        }
    }
}

/// Nearest-rank percentile over ascending-sorted integer-microsecond samples,
/// rank formula identical to `crates/d4lc-cli/src/live.rs::percentile_ms`.
fn percentile_us(samples: &mut [u128], percentile: usize) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    let rank = ((percentile * samples.len()).saturating_add(99) / 100).saturating_sub(1);
    samples[rank.min(samples.len().saturating_sub(1))]
}

fn us_to_ms(value: u128) -> f64 {
    value as f64 / 1000.0
}

fn fmt_ms(value: u128) -> String {
    format!("{:.3}", us_to_ms(value))
}

struct Counters {
    lookups: u64,
    accepted: u64,
    rejected: u64,
    candidate_sum: u64,
    wanted: u64,
    unwanted: u64,
    trade_matches: u64,
}

fn accumulate_counters(
    result: &ItemTooltipParseResult,
    evaluation: &d4lc_rules::RuleEvaluationResult,
    counters: &mut Counters,
) {
    let mut trace_lookup = |fuzzy: &Option<d4lc_fuzzy::FuzzyMatchResult>| {
        if let Some(fuzzy) = fuzzy {
            counters.lookups += 1;
            counters.candidate_sum += fuzzy.candidate_count as u64;
            if fuzzy.accepted {
                counters.accepted += 1;
            } else {
                counters.rejected += 1;
            }
        }
    };
    for affix in &result.affixes {
        trace_lookup(&affix.fuzzy);
    }
    if let Some(power) = &result.legendary_power {
        trace_lookup(&power.fuzzy);
    }
    for rune in &result.runes {
        counters.lookups += 1;
        counters.candidate_sum += rune.fuzzy.candidate_count as u64;
        if rune.fuzzy.accepted {
            counters.accepted += 1;
        } else {
            counters.rejected += 1;
        }
    }
    for sigil in &result.sigils {
        counters.lookups += 1;
        counters.candidate_sum += sigil.fuzzy.candidate_count as u64;
        if sigil.fuzzy.accepted {
            counters.accepted += 1;
        } else {
            counters.rejected += 1;
        }
    }
    match evaluation.final_decision {
        d4lc_rules::RuleDecision::Wanted => counters.wanted += 1,
        d4lc_rules::RuleDecision::Unwanted => counters.unwanted += 1,
        _ => {}
    }
    if evaluation.trade_marker.is_some() {
        counters.trade_matches += 1;
    }
}

fn measure_scenario(
    fixtures: &[TooltipFixture],
    base_indexes: &FuzzyIndexes,
    config: &RuleBenchmarkConfig,
    cold_cache: bool,
) -> ScenarioMetrics {
    let capacity = fixtures.len() * ITERATIONS;
    let mut metrics = ScenarioMetrics::new(capacity);
    for _ in 0..ITERATIONS {
        for fixture in fixtures {
            // Cold-cache runs clone the whole index set (cache empty) so every
            // fuzzy lookup performs the full candidate scan, outside the
            // timed window. Warm-cache runs share the loaded indexes, which
            // mirrors the upstream per-session memoization dictionaries.
            let owned_cold_indexes;
            let indexes: &FuzzyIndexes = if cold_cache {
                owned_cold_indexes = base_indexes.clone();
                &owned_cold_indexes
            } else {
                base_indexes
            };
            let parse_start_allocs = STATS.snapshot();
            let parse_start = std::time::Instant::now();
            // Upstream per-tooltip conversion surface (OcrHandler.ConvertToPower
            // / ConvertToItemType) over the upper-tooltip raw text, alongside
            // the segment-oriented fuzzy parse.
            let upper_text = fixture
                .segments
                .iter()
                .find(|segment| segment.kind == TooltipOcrSegmentKind::UpperTooltip)
                .map(|segment| segment.raw_text.clone());
            let item_power = upper_text.as_deref().map(convert_to_item_power);
            let item_type = upper_text
                .as_deref()
                .map(|text| convert_to_item_type(text, indexes));
            std::hint::black_box((&item_power, &item_type));
            let parse_result =
                parse_item_tooltip_from_ocr_segments_with_fuzzy(&fixture.segments, indexes);
            let parse_elapsed = parse_start.elapsed();
            let after_parse_allocs = STATS.snapshot();
            std::hint::black_box(&parse_result);

            let rules_start = std::time::Instant::now();
            let evaluation = evaluate_rules(&parse_result, config);
            let rules_elapsed = rules_start.elapsed();
            let after_rules_allocs = STATS.snapshot();
            std::hint::black_box(&evaluation);

            metrics.parse_us.push(parse_elapsed.as_micros());
            metrics.rules_us.push(rules_elapsed.as_micros());
            metrics
                .total_us
                .push(parse_elapsed.as_micros() + rules_elapsed.as_micros());
            let parse_delta = after_parse_allocs.delta_since(parse_start_allocs);
            let rules_delta = after_rules_allocs.delta_since(after_parse_allocs);
            metrics.parse_allocs += parse_delta.alloc_count;
            metrics.parse_alloc_bytes += parse_delta.alloc_bytes;
            metrics.rules_allocs += rules_delta.alloc_count;
            metrics.rules_alloc_bytes += rules_delta.alloc_bytes;

            let mut counters = Counters {
                lookups: 0,
                accepted: 0,
                rejected: 0,
                candidate_sum: 0,
                wanted: 0,
                unwanted: 0,
                trade_matches: 0,
            };
            accumulate_counters(&parse_result, &evaluation, &mut counters);
            metrics.fuzzy_lookups += counters.lookups;
            metrics.fuzzy_accepted += counters.accepted;
            metrics.fuzzy_rejected += counters.rejected;
            metrics.fuzzy_candidate_sum += counters.candidate_sum;
            metrics.wanted_decisions += counters.wanted;
            metrics.unwanted_decisions += counters.unwanted;
            metrics.trade_matches += counters.trade_matches;
        }
    }
    metrics
}

fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.clone())
}

fn first_file_line(path: &str, prefix: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find(|line| line.contains(prefix)).map(|line| {
        line.split(':')
            .next_back()
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    })
}

struct EnvironmentInfo {
    os: String,
    kernel: String,
    cpu: String,
    cores: usize,
    ram_gb: u64,
    session: String,
    display: String,
    repo_commit: String,
}

fn environment_info(root: &std::path::Path) -> EnvironmentInfo {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("PRETTY_NAME="))
                .map(|line| {
                    line.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
        })
        .unwrap_or_else(|| "unavailable".to_string());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| "unavailable".to_string());
    let cpu =
        first_file_line("/proc/cpuinfo", "model name").unwrap_or_else(|| "unavailable".to_string());
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(0);
    let ram_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let rest = line.strip_prefix("MemTotal:")?;
                rest.trim()
                    .trim_end_matches(" kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or(0);
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unavailable".to_string());
    let display = std::env::var("WAYLAND_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .unwrap_or_else(|_| "unavailable".to_string());
    let repo_commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    EnvironmentInfo {
        os,
        kernel,
        cpu,
        cores,
        ram_gb: ram_kb / (1024 * 1024),
        session,
        display,
        repo_commit,
    }
}

fn write_report(
    label: &str,
    cold: &ScenarioMetrics,
    warm: &ScenarioMetrics,
    fixture_set: &[TooltipFixture],
    root: &std::path::Path,
) -> std::path::PathBuf {
    let mut total_cold = cold.total_us.clone();
    let cold_p95_total = percentile_us(&mut total_cold, 95);
    let env = environment_info(root);
    let percentiles_ms = |samples: &[u128]| {
        let mut samples = samples.to_vec();
        json!({
            "p50": us_to_ms(percentile_us(&mut samples, 50)),
            "p95": us_to_ms(percentile_us(&mut samples, 95)),
            "p99": us_to_ms(percentile_us(&mut samples, 99)),
        })
    };
    let scenario = |name: &str, metrics: &ScenarioMetrics, cold_cache: bool| {
        json!({
            "name": name,
            "cold_cache": cold_cache,
            "samples": metrics.total_us.len(),
            "timings_ms": {
                "parse": percentiles_ms(&metrics.parse_us),
                "rules": percentiles_ms(&metrics.rules_us),
                "parse_plus_rules": percentiles_ms(&metrics.total_us),
            },
            "allocations": {
                "parse": { "count": metrics.parse_allocs, "bytes": metrics.parse_alloc_bytes },
                "rules": { "count": metrics.rules_allocs, "bytes": metrics.rules_alloc_bytes },
            },
            "counters": {
                "fuzzy_lookups": metrics.fuzzy_lookups,
                "fuzzy_accepted": metrics.fuzzy_accepted,
                "fuzzy_rejected": metrics.fuzzy_rejected,
                "fuzzy_candidate_sum": metrics.fuzzy_candidate_sum,
                "avg_candidates_per_lookup": if metrics.fuzzy_lookups > 0 {
                    metrics.fuzzy_candidate_sum as f64 / metrics.fuzzy_lookups as f64
                } else {
                    0.0
                },
                "wanted_decisions": metrics.wanted_decisions,
                "unwanted_decisions": metrics.unwanted_decisions,
                "trade_matches": metrics.trade_matches,
            },
        })
    };
    let report = json!({
        "schema_version": 1,
        "command": "cargo test -p d4lc-parse --release --test perf_parse_rules_latency -- \
            --ignored --nocapture --test-threads=1",
        "label": label,
        "fixture_count": fixture_set.len(),
        "fixtures": fixture_set
            .iter()
            .map(|fixture| json!({ "name": fixture.name, "category": fixture.category }))
            .collect::<Vec<_>>(),
        "iterations": ITERATIONS,
        "counted_samples_per_scenario": cold.total_us.len(),
        "interval_ms": 0,
        "percentile_method": "nearest-rank",
        "sample_resolution": "integer microseconds per sample, reported in milliseconds \
            (rank formula identical to live.rs::percentile_ms)",
        "environment": {
            "os": env.os,
            "kernel": env.kernel,
            "cpu": env.cpu,
            "cores": env.cores,
            "ram_gb": env.ram_gb,
            "session": env.session,
            "display": env.display,
            "build_mode": "release (cargo test --release, opt-level 3)",
            "repo_commit": env.repo_commit,
            "preset": "n/a (library-stage parse+rules measurement, no capture)",
        },
        "warmup": { "discarded_invocations": 1, "counted_samples_min": 60 },
        "allocations": {
            "peak_live_bytes": STATS.snapshot().max_live_bytes,
            "note": "peak_live_bytes is a global high-water mark across the measured invocation",
        },
        "scenarios": [
            scenario("parse_cold_fuzzy", cold, true),
            scenario("parse_warm_fuzzy", warm, false),
        ],
        "budgets": {
            "parse_plus_rules_hotpath": {
                "budget_p95_ms": BUDGET_P95_MS,
                "measured_p95_ms": us_to_ms(cold_p95_total),
                "pass": us_to_ms(cold_p95_total) < BUDGET_P95_MS,
                "provisional": false,
                "basis": "cold-cache scenario (worst case; every fuzzy lookup scans the full \
                    candidate set)",
            },
        },
    });
    let json = serde_json::to_string_pretty(&report).unwrap_or_else(|error| panic!("{error}"));

    let output_dir = root.join("debug-output/perf-parse-rules");
    std::fs::create_dir_all(&output_dir)
        .unwrap_or_else(|error| panic!("create debug-output dir: {error}"));
    let output_path = output_dir.join(format!("{label}.json"));
    std::fs::write(&output_path, &json)
        .unwrap_or_else(|error| panic!("write report {}: {error}", output_path.display()));
    output_path
}

/// Full live-measurement run: one discarded warmup invocation, then the
/// counted cold-cache and warm-cache scenarios. Excluded from the default
/// suite (`#[ignore]`); run explicitly with `--ignored`.
#[test]
#[ignore = "benchmark run; invoke explicitly with --ignored"]
fn full_latency_report() {
    let root = workspace_root();
    let repo = AssetRepository::new(root.clone());
    let base_indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let fixture_set = fixtures();
    let fixture_count = fixture_set.len();

    // Warmup invocation (discarded): also derives the rule config from the
    // real parsed ids so matched rule branches use canonical data ids.
    let mut fixture_results = Vec::with_capacity(fixture_count);
    for fixture in &fixture_set {
        let result =
            parse_item_tooltip_from_ocr_segments_with_fuzzy(&fixture.segments, &base_indexes);
        fixture_results.push((fixture.name, result));
    }
    let config = build_rule_config(&fixture_results);
    // Sanity: the amulet fixture must evaluate to Wanted with a trade match.
    let amulet = &fixture_results[0].1;
    let amulet_evaluation = evaluate_rules(amulet, &config);
    assert_eq!(
        amulet_evaluation.final_decision,
        d4lc_rules::RuleDecision::Wanted,
        "amulet fixture must exercise the wanted multi-build branch"
    );
    assert!(
        amulet_evaluation.trade_marker.is_some(),
        "amulet fixture must exercise the trade-match branch"
    );
    drop(fixture_results);

    let cold = measure_scenario(&fixture_set, &base_indexes, &config, true);
    let warm = measure_scenario(&fixture_set, &base_indexes, &config, false);

    let label = std::env::var("D4LC_PERF_LABEL").unwrap_or_else(|_| "run".to_string());
    let report_path = write_report(&label, &cold, &warm, &fixture_set, &root);

    let mut parse_cold = cold.parse_us.clone();
    let mut rules_cold = cold.rules_us.clone();
    let mut total_cold = cold.total_us.clone();
    let mut parse_warm = warm.parse_us.clone();
    let mut rules_warm = warm.rules_us.clone();
    let mut total_warm = warm.total_us.clone();
    println!(
        "parse + rules latency report ({label}): {}",
        report_path.display()
    );
    println!(
        "cold parse  p50/p95/p99 ms: {} / {} / {}",
        fmt_ms(percentile_us(&mut parse_cold, 50)),
        fmt_ms(percentile_us(&mut parse_cold, 95)),
        fmt_ms(percentile_us(&mut parse_cold, 99))
    );
    println!(
        "cold rules  p50/p95/p99 ms: {} / {} / {}",
        fmt_ms(percentile_us(&mut rules_cold, 50)),
        fmt_ms(percentile_us(&mut rules_cold, 95)),
        fmt_ms(percentile_us(&mut rules_cold, 99))
    );
    println!(
        "cold total  p50/p95/p99 ms: {} / {} / {} (budget p95 < {BUDGET_P95_MS})",
        fmt_ms(percentile_us(&mut total_cold, 50)),
        fmt_ms(percentile_us(&mut total_cold, 95)),
        fmt_ms(percentile_us(&mut total_cold, 99))
    );
    println!(
        "warm parse  p50/p95/p99 ms: {} / {} / {}",
        fmt_ms(percentile_us(&mut parse_warm, 50)),
        fmt_ms(percentile_us(&mut parse_warm, 95)),
        fmt_ms(percentile_us(&mut parse_warm, 99))
    );
    println!(
        "warm rules  p50/p95/p99 ms: {} / {} / {}",
        fmt_ms(percentile_us(&mut rules_warm, 50)),
        fmt_ms(percentile_us(&mut rules_warm, 95)),
        fmt_ms(percentile_us(&mut rules_warm, 99))
    );
    println!(
        "warm total  p50/p95/p99 ms: {} / {} / {}",
        fmt_ms(percentile_us(&mut total_warm, 50)),
        fmt_ms(percentile_us(&mut total_warm, 95)),
        fmt_ms(percentile_us(&mut total_warm, 99))
    );
    println!(
        "allocs per tooltip (cold): parse {} / rules {}; bytes: parse {} / rules {}",
        cold.parse_allocs / cold.total_us.len() as u64,
        cold.rules_allocs / cold.total_us.len() as u64,
        cold.parse_alloc_bytes / cold.total_us.len() as u64,
        cold.rules_alloc_bytes / cold.total_us.len() as u64
    );
}

/// Always-on guard for the harness's decisive math: the nearest-rank formula
/// must match the `live.rs::percentile_ms` oracle exactly, and the fixture
/// pipeline must produce the expected canonical ids and decisions so the
/// measured workload stays the parity-tested one.
#[test]
fn harness_percentile_and_fixture_sanity() {
    // Oracle (live.rs test): samples [1,3,5,7,9] -> p50 5, p95 9.
    let mut samples = vec![9_u128, 1, 7, 3, 5];
    assert_eq!(percentile_us(&mut samples, 50), 5);
    assert_eq!(percentile_us(&mut samples, 95), 9);
    // Nearest-rank clamp at the top of the sample list.
    let mut two = vec![4_u128, 2];
    assert_eq!(percentile_us(&mut two, 50), 2);
    assert_eq!(percentile_us(&mut two, 95), 4);

    let root = workspace_root();
    let repo = AssetRepository::new(root);
    let indexes = FuzzyIndexes::load(&repo, "enUS").unwrap_or_else(|error| panic!("{error}"));
    let fixture_set = fixtures();
    assert_eq!(fixture_set.len(), 10);
    let mut fixture_results = Vec::with_capacity(fixture_set.len());
    for fixture in &fixture_set {
        let result = parse_item_tooltip_from_ocr_segments_with_fuzzy(&fixture.segments, &indexes);
        fixture_results.push((fixture.name, result));
    }
    let config = build_rule_config(&fixture_results);
    let by_name = |name: &str| {
        fixture_results
            .iter()
            .find(|(fixture_name, _)| *fixture_name == name)
            .map(|(_, result)| result)
            .unwrap_or_else(|| panic!("missing fixture {name}"))
    };

    // Canonical id parity (same expectations as the parser/rule unit tests).
    let aspect = by_name("legendary_weapon_with_aspect");
    assert_eq!(
        aspect
            .legendary_power
            .as_ref()
            .and_then(|power| power.power_id.as_deref()),
        Some("Legendary_Generic_110")
    );
    let unique = by_name("unique_boots_with_socket");
    assert_eq!(
        unique
            .legendary_power
            .as_ref()
            .and_then(|power| power.power_id.as_deref()),
        Some("Boots_Unique_Barb_100_x2")
    );
    let rune = by_name("rune_item");
    assert_eq!(
        rune.runes.first().and_then(|item| item.rune_id.as_deref()),
        Some("Item_Rune_Condition_OnMove")
    );
    let sigil = by_name("nightmare_sigil");
    assert_eq!(
        sigil
            .sigils
            .first()
            .and_then(|item| item.sigil_id.as_deref()),
        Some("World_DGN_Kehj_AbandonedMineworks")
    );
    let no_match = by_name("no_match_affixes");
    // Upstream affix matching has no acceptance threshold (0.0), so junk
    // affix text still records a best-scoring (unwanted) winner; the rune
    // lookup carries the 75 threshold and must reject the junk rune line.
    let no_match_rune = no_match
        .runes
        .first()
        .unwrap_or_else(|| panic!("missing junk rune"));
    assert!(!no_match_rune.fuzzy.accepted);
    assert_eq!(no_match_rune.rune_id, None);
    // Rule decisions: wanted + trade match for the amulet, unwanted for the
    // no-match fixture, no trade marker for the ring.
    let amulet = by_name("legendary_amulet_four_affixes");
    let amulet_evaluation = evaluate_rules(amulet, &config);
    assert_eq!(
        amulet_evaluation.final_decision,
        d4lc_rules::RuleDecision::Wanted
    );
    assert!(amulet_evaluation.trade_marker.is_some());
    let no_match_evaluation = evaluate_rules(no_match, &config);
    assert_eq!(
        no_match_evaluation.final_decision,
        d4lc_rules::RuleDecision::Unwanted
    );
    let ring = by_name("magic_ring_two_affixes");
    let ring_evaluation = evaluate_rules(ring, &config);
    assert!(
        ring_evaluation.trade_marker.is_none(),
        "ring fixture must exercise the trade no-match branch"
    );

    // The matched sigil requirement must resolve with the sigil fixture id.
    let sigil_evaluation = evaluate_rules(sigil, &config);
    assert!(
        !sigil_evaluation.explanation.branches.is_empty(),
        "sigil fixture must produce rule branches"
    );
    // Ensure the non-affix evaluation path is actually exercised via the
    // public API used by the benchmark (compile-time consumer proof).
    let parsed_non_affix = non_affix_for_rules(aspect);
    assert!(!parsed_non_affix.is_empty());
    let non_affix = evaluate_non_affix_multi_build(
        &parsed_non_affix,
        &config.multi_build,
        &config.non_affix_presets,
    );
    assert!(
        non_affix
            .matches
            .iter()
            .any(|matched| matched.kind == RuleNonAffixKind::Aspect && matched.color == "Blue"),
        "aspect requirement must match under the first build color"
    );
}
