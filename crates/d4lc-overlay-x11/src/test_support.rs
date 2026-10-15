use crate::{OverlayFrame, OverlayHandle, OverlayRenderOp};
use d4lc_core::Rect;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(crate) fn display_env_lock() -> &'static Mutex<()> {
    static DISPLAY_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    DISPLAY_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn save_and_remove_display() -> Option<String> {
    let saved = std::env::var("DISPLAY").ok();
    std::env::remove_var("DISPLAY");
    saved
}

pub(crate) fn restore_display(saved: Option<String>) {
    if let Some(value) = saved {
        std::env::set_var("DISPLAY", value);
    } else {
        std::env::remove_var("DISPLAY");
    }
}

pub(crate) fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates| crates.parent())
        .unwrap_or_else(|| panic!("could not resolve workspace root"))
        .to_path_buf()
}

pub(crate) fn collect_render_ops(
    overlay: &mut OverlayHandle,
    sequence: u64,
    descriptors: Vec<d4lc_rules::RuleOverlayDescriptor>,
) -> Vec<OverlayRenderOp> {
    overlay
        .update(OverlayFrame {
            sequence,
            tooltip_found: true,
            hotpath_ms: 5,
            cursor_window_relative: None,
            rule_descriptors: descriptors.into(),
            text_font_px: crate::DEFAULT_OVERLAY_FONT_PX,
        })
        .unwrap_or_else(|error| panic!("overlay update failed: {error}"));
    overlay.last_render_ops().to_vec()
}

pub(crate) fn write_render_trace(
    file_name: &str,
    traces: &[(
        &str,
        &[d4lc_rules::RuleOverlayDescriptor],
        &[OverlayRenderOp],
    )],
) {
    let json = serde_json::Value::Array(
        traces
            .iter()
            .map(|(name, descriptors, render_ops)| {
                serde_json::json!({
                    "mode": name,
                    "descriptors": descriptors.iter().map(rule_overlay_descriptor_json).collect::<Vec<_>>(),
                    "render_ops": render_ops.iter().map(render_op_json).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
    );
    let path = workspace_root()
        .join("debug-output/overlay")
        .join(file_name);
    std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        path,
        serde_json::to_string_pretty(&json).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

pub(crate) fn rule_overlay_descriptor_json(
    descriptor: &d4lc_rules::RuleOverlayDescriptor,
) -> serde_json::Value {
    serde_json::json!({
        "marker_type": format!("{:?}", descriptor.marker_type),
        "shape": format!("{:?}", descriptor.shape),
        "geometry": rect_json(descriptor.geometry),
        "color": descriptor.color,
        "text": descriptor.text,
        "geometry_source": descriptor.geometry_source,
        "source": {
            "upstream_file": descriptor.source.upstream_file,
            "upstream_method": descriptor.source.upstream_method,
            "branch": descriptor.source.branch,
        },
    })
}

pub(crate) fn render_op_json(operation: &OverlayRenderOp) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", operation.kind),
        "marker_type": format!("{:?}", operation.marker_type),
        "z_order_group": format!("{:?}", operation.z_order_group),
        "geometry": rect_json(operation.geometry),
        "color": operation.color,
        "text": operation.text,
        "geometry_source": operation.geometry_source,
        "source": operation.source.as_ref().map(|source| serde_json::json!({
            "upstream_file": source.upstream_file,
            "upstream_method": source.upstream_method,
            "branch": source.branch,
        })),
    })
}

pub(crate) fn rect_json(rect: Rect) -> serde_json::Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}
