//! d4lc-rules crate.

mod affix;
mod catalog;
mod import;
mod model;
mod non_affix;
mod overlay;
mod paragon;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod test_support;
mod trade;

pub use affix::{
    apply_item_power_and_value_filters, evaluate_affix_preset, evaluate_affix_preset_rows,
    evaluate_affix_preset_rows_with_thresholds, evaluate_multi_build_affix_presets,
    parse_affix_value_text, trace_affix_row, trace_affix_rows, AffixRowText,
};
pub use catalog::{AffixCatalog, CatalogEntry, CatalogKind, ClassFlags};
pub use d4lc_core::AffixRowId;
pub use import::{
    convert_d4builds_site_variants, convert_mobalytics_graphql_build, ensure_cache_filename_safe,
    export_d4builds_build_cache, export_infinity_builds_build_cache, export_mobalytics_build_cache,
    export_mobalytics_profile_cache, extract_infinity_builds_share_script,
    extract_infinity_builds_share_scripts, import_d2core_build_variant,
    import_d4builds_build_variant, import_infinity_builds_build_variant,
    import_infinity_builds_share_html, import_maxroll_build_profile,
    import_mobalytics_build_variant, load_available_d2core_builds, load_available_d2core_variants,
    load_available_d4builds_builds, load_available_d4builds_variants,
    load_available_infinity_builds_builds, load_available_infinity_builds_variants,
    load_available_maxroll_builds, load_available_maxroll_variants,
    load_available_mobalytics_builds, load_available_mobalytics_profiles,
    load_available_mobalytics_variants, parse_d4builds_site_paragon_boards,
    parse_mobalytics_graphql_build, parse_mobalytics_graphql_build_with_warnings,
    parse_mobalytics_profile_state, remove_mobalytics_profile, unix_ms_to_utc_timestamp,
    D4BuildsAffixFile, D4BuildsBuildFile, D4BuildsConversionLookups, D4BuildsConvertedBuild,
    D4BuildsConvertedVariant, D4BuildsParagonBoardFile, D4BuildsSiteAffix, D4BuildsSiteAffixGroup,
    D4BuildsSiteBuild, D4BuildsSiteParagonBoard, D4BuildsSiteVariant, D4BuildsVariantFile,
    ImportBuildDescriptor, ImportBuildList, ImportColors, ImportLocalizedData,
    ImportLocalizedDataError, ImportLocalizedDataState, ImportUnmappedEntry,
    ImportVariantDescriptor, ImportWarning, ImportWorkflowResult, ImportedItemAffixes,
    ImportedParagonBoard, ImportedParagonRoute, InfinityBuildsAffix, InfinityBuildsBuild,
    InfinityBuildsBuildVariant, InfinityBuildsShareImport, LocalizedDataComponent,
    MobalyticsAffixEntry, MobalyticsBuildFile, MobalyticsConversionLookups,
    MobalyticsConvertedBuild, MobalyticsConvertedVariant, MobalyticsParagonBoardFile,
    MobalyticsProfileFile, MobalyticsProfileVariant, MobalyticsVariantFile,
    MOBALYTICS_SHOW_BROWSER_DEFAULT,
};
pub use model::{
    AffixMatchPass, AffixPreset, AffixRequirement, AffixRowVerdict, AffixRuleRow, AffixValueFilter,
    AffixValueThreshold, MultiBuildEntry, MultiBuildSettings, NonAffixMatch,
    NonAffixMultiBuildResult, NonAffixPreset, NonAffixRequirement, NonAffixRowVerdict,
    ParsedAffixForRules, ParsedAffixKind, ParsedItemForRules, ParsedNonAffixForRules,
    RuleAffixMatch, RuleBranchExplanation, RuleCheck, RuleCheckKind, RuleCheckStatus, RuleDecision,
    RuleEvaluationResult, RuleExplanation, RuleFilterSettings, RuleMatchTrace, RuleMissingAffix,
    RuleNonAffixKind, RulePowerKind, RulePowerMatch, RuleRejection, RuleSourceNote, RuleTraceEntry,
    RuleTradeMarker, RuleValueCheck, SigilDisplayMode,
};
pub use non_affix::{
    evaluate_non_affix_multi_build, evaluate_non_affix_preset_rows, find_power_match,
    find_rune_match, find_sigil_match,
};
pub use overlay::{
    apply_overlay_descriptors, build_live_overlay_descriptors, build_overlay_descriptors,
    is_upstream_red, upstream_affix_marker_shape, upstream_marker_box, OverlayColor, OverlayFrame,
    RuleAffixOverlayLocation, RuleIdOverlayLocation, RuleOverlayDescriptor, RuleOverlayLayout,
    RuleOverlayMarkerMode, RuleOverlayMarkerType, RuleOverlaySettings, RuleOverlayShape,
    RuleOverlayTarget, RuleOverlayZOrderGroup, DEFAULT_OVERLAY_FONT_PX, UPSTREAM_MARKER_LENGTH,
    UPSTREAM_MARKER_RADIUS,
};
pub use paragon::{
    build_paragon_overlay_descriptors, ParagonColor, ParagonOverlayElementKind,
    ParagonOverlayLayout, ParagonOverlayMode, ParagonOverlayState,
};
pub use trade::{apply_trade_item_lookup, find_trade_marker, TradeAffixRequirement, TradeItemRule};
