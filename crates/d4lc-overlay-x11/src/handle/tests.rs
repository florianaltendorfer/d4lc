use super::*;
use crate::menu::{OVERLAY_MENU_HEIGHT, OVERLAY_MENU_WIDTH};
use crate::test_support::*;

#[test]
fn set_window_geometry_moves_the_surface_and_its_menu_zone() {
    let config = OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        },
        show_overlay_icon: true,
        overlay_icon_pos_x: 500,
        overlay_icon_pos_y: 100,
        menu_locked: true,
    };
    let mut handle = OverlayHandle::create_headless(config, "test");
    let moved = Rect {
        x: -1920,
        y: 40,
        width: 2000,
        height: 1000,
    };
    handle
        .set_window_geometry(moved)
        .unwrap_or_else(|error| panic!("reposition: {error}"));
    assert_eq!(handle.status().window_geometry, moved);
    // The zone stays window-relative: a per-mille share of the new size.
    assert_eq!(handle.status().interaction_zone.x, 1000);
    assert_eq!(handle.status().interaction_zone.y, 100);
    assert!(handle
        .set_window_geometry(Rect { width: 0, ..moved })
        .is_err());
}

#[test]
fn zero_geometry_returns_error() {
    let err = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    });
    match err {
        Err(msg) => assert!(
            msg.contains("overlay window geometry must be non-zero"),
            "unexpected error message: {msg}"
        ),
        Ok(_) => panic!("create_overlay with zero geometry must return Err"),
    }
}

/// When DISPLAY is unset this test verifies headless fallback regardless of
/// whether the `real-x11` feature is compiled in.
#[test]
fn falls_back_gracefully_without_display() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let geometry = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let result = create_overlay(OverlayConfig {
        window_geometry: geometry,
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    });

    restore_display(saved);

    let handle = result.unwrap_or_else(|e| panic!("create_overlay failed: {e}"));
    // Without DISPLAY we always get HeadlessNoop (even with real-x11 feature).
    assert_eq!(handle.status().backend, OverlayBackend::HeadlessNoop);
    assert!(handle.status().enabled);
    assert!(!handle.status().click_through);
}

#[test]
fn creates_headless_noop_overlay_status() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let geometry = Rect {
        x: 1920,
        y: 0,
        width: 1920,
        height: 1080,
    };

    let saved = save_and_remove_display();

    let overlay = create_overlay(OverlayConfig {
        window_geometry: geometry,
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    assert_eq!(overlay.status().backend, OverlayBackend::HeadlessNoop);
    assert!(overlay.status().enabled);
    assert_eq!(overlay.status().window_geometry, geometry);
    assert_eq!(
        overlay.status().interaction_zone,
        Rect {
            x: 0,
            y: 0,
            width: OVERLAY_MENU_WIDTH,
            height: OVERLAY_MENU_HEIGHT,
        }
    );
    assert!(overlay.status().menu_locked);
    assert!(!overlay.status().menu_visible);
    assert!(!overlay.status().transparent);
    assert!(!overlay.status().always_on_top);
    assert!(!overlay.status().click_through);
    assert!(overlay.status().limitation.is_some());
}

#[test]
fn update_records_latest_diagnostic_frame() {
    let _display_lock = display_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let saved = save_and_remove_display();

    let mut overlay = create_overlay(OverlayConfig {
        window_geometry: Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        },
        show_overlay_icon: false,
        overlay_icon_pos_x: 0,
        overlay_icon_pos_y: 0,
        menu_locked: true,
    })
    .unwrap_or_else(|error| panic!("overlay creation failed: {error}"));

    restore_display(saved);

    overlay
        .update(OverlayFrame {
            sequence: 7,
            tooltip_found: true,
            hotpath_ms: 11,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));

    assert_eq!(
        overlay.last_frame(),
        Some(&OverlayFrame {
            sequence: 7,
            tooltip_found: true,
            hotpath_ms: 11,
            cursor_window_relative: None,
            rule_descriptors: std::sync::Arc::from([]),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
    );
}
