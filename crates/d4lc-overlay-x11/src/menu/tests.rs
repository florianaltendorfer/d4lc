use super::*;
use crate::test_support::*;
use crate::*;

#[test]
fn interaction_zone_scales_from_configured_icon_position() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        },
        show_overlay_icon: true,
        overlay_icon_pos_x: 500,
        overlay_icon_pos_y: 250,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    assert_eq!(
        overlay.status().interaction_zone,
        Rect {
            x: 100,
            y: 25,
            width: OVERLAY_MENU_WIDTH,
            height: OVERLAY_MENU_HEIGHT,
        }
    );
}

#[test]
fn hover_lock_timer_toggles_menu_descriptors_render_in_both_states() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    let base = Instant::now();
    let frame = OverlayFrame {
        sequence: 10,
        tooltip_found: true,
        hotpath_ms: 6,
        cursor_window_relative: Some(Point { x: 10, y: 10 }),
        rule_descriptors: vec![d4lc_rules::RuleOverlayDescriptor {
            target: d4lc_rules::RuleOverlayTarget::Affix {
                affix_id: "Affix_AttackSpeed".to_string(),
                row_id: Some(d4lc_core::AffixRowId(0)),
            },
            marker_type: d4lc_rules::RuleOverlayMarkerType::Affix,
            shape: d4lc_rules::RuleOverlayShape::Rectangle,
            geometry: Rect {
                x: 130,
                y: 240,
                width: 20,
                height: 20,
            },
            color: "Gold".into(),
            text: None,
            text_origin: None,
            z_order_group: d4lc_rules::RuleOverlayZOrderGroup::Marker,
            geometry_source: "full_window_affix_marker_geometry".into(),
            source: d4lc_rules::RuleSourceNote::new(
                "D4Companion.Services/OverlayHandler.cs",
                "OverlayHandler.DrawGraphicsAffixes",
                "affix marker mode and shape selection",
            ),
        }]
        .into(),
        text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
    };

    overlay
        .update_at(frame.clone(), base)
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));
    assert!(!overlay.status().menu_locked);
    assert!(overlay.status().menu_visible);
    assert!(overlay
        .last_render_ops()
        .iter()
        .any(|operation| { operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Affix }));
    assert!(overlay
        .last_render_ops()
        .iter()
        .any(|operation| operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Menu));

    overlay
        .update_at(
            frame,
            base + std::time::Duration::from_millis(OVERLAY_MENU_LOCK_TIMER_MS as u64 + 1),
        )
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));
    assert!(overlay.status().menu_locked);
    assert!(overlay
        .last_render_ops()
        .iter()
        .any(|operation| { operation.marker_type == d4lc_rules::RuleOverlayMarkerType::Affix }));
}

#[test]
fn leaving_interaction_zone_resets_lock_progress() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
        show_overlay_icon: true,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: false,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    let base = Instant::now();
    overlay
        .update_at(
            OverlayFrame {
                sequence: 11,
                tooltip_found: false,
                hotpath_ms: 4,
                cursor_window_relative: Some(Point { x: 10, y: 10 }),
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
            },
            base,
        )
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    overlay
        .update_at(
            OverlayFrame {
                sequence: 12,
                tooltip_found: false,
                hotpath_ms: 4,
                cursor_window_relative: Some(Point { x: 10, y: 10 }),
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
            },
            base + std::time::Duration::from_millis(500),
        )
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    assert!(overlay
        .last_render_ops()
        .iter()
        .any(|operation| operation.color == "Gold"
            && operation.kind == OverlayRenderKind::FilledRect));

    overlay
        .update_at(
            OverlayFrame {
                sequence: 13,
                tooltip_found: false,
                hotpath_ms: 4,
                cursor_window_relative: Some(Point { x: 200, y: 200 }),
                rule_descriptors: std::sync::Arc::from([]),
                text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
            },
            base + std::time::Duration::from_millis(600),
        )
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    assert!(!overlay.status().menu_visible);
    assert!(!overlay
        .last_render_ops()
        .iter()
        .any(|operation| operation.color == "Gold"
            && operation.kind == OverlayRenderKind::FilledRect));
}
