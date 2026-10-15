use crate::roots::Roots;
use crate::to_json_text;
use d4lc_capture::{
    capture_manual_region, capture_manual_region_native, capture_selected_window,
    capture_window_or_manual_region_with_backend, discover_x11_windows, read_cursor_state,
    CaptureBackend, CaptureResult, CaptureSource, CursorState, ScaleFactor, WindowCandidate,
    WindowQuery, WindowSelection,
};
use d4lc_core::Rect;
use d4lc_debug::{write_capture_png, write_debug_text};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureArgs {
    pub match_title: Option<String>,
    pub class_name: Option<String>,
    pub pid: Option<u32>,
    pub exact_title: bool,
    pub output_path: Option<PathBuf>,
    pub capture_output_path: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub manual_region: Option<Rect>,
    pub backend: Option<String>,
    pub preset: Option<String>,
    pub delay_millis: Option<u64>,
}

impl CaptureArgs {
    fn window_query(&self) -> WindowQuery {
        WindowQuery {
            title: self.match_title.clone(),
            class_name: self.class_name.clone(),
            pid: self.pid,
            exact_title: self.exact_title,
        }
    }

    fn defaulted_window_query(&self) -> WindowQuery {
        self.window_query().with_default_diablo_title()
    }

    fn selected_output_path(&self) -> Option<&Path> {
        self.capture_output_path
            .as_deref()
            .or(self.output_path.as_deref())
    }

    fn sleep_before_capture(&self) {
        if let Some(delay_millis) = self.delay_millis {
            thread::sleep(Duration::from_millis(delay_millis));
        }
    }

    fn capture_backend(&self) -> Result<CaptureBackend, String> {
        match self.backend.as_deref() {
            Some(value) => CaptureBackend::parse(value),
            None => Ok(CaptureBackend::X11Command),
        }
    }
}

pub fn x11_windows(args: &CaptureArgs) -> Result<String, String> {
    let selection = discover_x11_windows(&args.window_query())?;
    Ok(window_selection_json(&selection))
}

pub fn capture_window_raw(roots: &Roots, args: &CaptureArgs) -> Result<String, String> {
    args.sleep_before_capture();
    let result = if let Some(region) = args.manual_region {
        capture_manual_region_with_backend(region, args.capture_backend()?)?
    } else if args.backend.is_some() {
        capture_window_or_manual_region_with_backend(
            &args.defaulted_window_query(),
            None,
            args.capture_backend()?,
        )?
    } else {
        capture_selected_window(&args.defaulted_window_query())?
    };
    let output_path = roots.work_path(
        args.selected_output_path()
            .unwrap_or(Path::new("debug-output/capture/window.png")),
    );
    write_capture_png(&output_path, &result.frame)?;
    Ok(capture_result_json(
        &result,
        &relative_or_display(&roots.work, &output_path),
    ))
}

fn capture_manual_region_with_backend(
    region: Rect,
    backend: CaptureBackend,
) -> Result<CaptureResult, String> {
    match backend {
        CaptureBackend::Auto => match capture_manual_region_native(region) {
            Ok(result) => Ok(result),
            Err(native_error) => {
                let mut result = capture_manual_region(region)?;
                result.fallback_reason = Some(native_error);
                Ok(result)
            }
        },
        CaptureBackend::X11Native => capture_manual_region_native(region),
        CaptureBackend::X11Command => capture_manual_region(region),
    }
}

pub fn cursor_position(args: &CaptureArgs) -> Result<String, String> {
    let reference_rect = if let Some(region) = args.manual_region {
        Some(region)
    } else {
        discover_x11_windows(&args.defaulted_window_query())?
            .selected
            .map(|candidate| candidate.info.rect)
    };
    let cursor = read_cursor_state(reference_rect)?;
    Ok(cursor_json(&cursor, reference_rect))
}

pub fn capture_debug_bundle(roots: &Roots, args: &CaptureArgs) -> Result<String, String> {
    args.sleep_before_capture();
    let output_dir = roots.work_path(
        args.output_dir
            .as_deref()
            .unwrap_or(Path::new("debug-output/capture-bundle")),
    );
    let result = capture_window_or_manual_region_with_backend(
        &args.window_query(),
        args.manual_region,
        args.capture_backend()?,
    )?;
    let cursor = read_cursor_state(Some(result.geometry)).ok();
    let frame_path = output_dir.join("frame.png");
    let metadata_path = output_dir.join("capture.json");
    write_capture_png(&frame_path, &result.frame)?;

    let metadata = debug_bundle_json(args, &result, cursor.as_ref(), &frame_path, &metadata_path);
    write_debug_text(&metadata_path, &to_json_text(&metadata))?;

    Ok(to_json_text(&json!({
        "schema_version": 1,
        "output_dir": relative_or_display(&roots.work, &output_dir),
        "frame": relative_or_display(&roots.work, &frame_path),
        "metadata": relative_or_display(&roots.work, &metadata_path),
    })))
}

fn window_selection_json(selection: &WindowSelection) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "selected": selection.selected.as_ref().map(window_candidate_json),
        "candidates": selection.candidates.iter().map(window_candidate_json).collect::<Vec<_>>(),
    }))
}

fn capture_result_json(result: &CaptureResult, output_path: &str) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "capture": capture_json(result),
        "output": output_path,
    }))
}

fn capture_json(result: &CaptureResult) -> Value {
    let (source, window) = match &result.source {
        CaptureSource::Window(candidate) => ("window", Some(window_candidate_json(candidate))),
        CaptureSource::ManualRegion(_) => ("manual_region", None),
        CaptureSource::PortalStream(_) => ("portal_stream", None),
    };
    json!({
        "backend": result.backend.to_string(),
        "fallback_reason": result.fallback_reason,
        "source": source,
        "geometry": result.geometry,
        "scale": scale_json(result.scale),
        "capture_scope": result.capture_scope.to_string(),
        "capture_method": result.capture_method.to_string(),
        "roi_origin": result.roi_origin.to_string(),
        "requested_capture_rect": result.requested_capture_rect,
        "actual_capture_rect": result.actual_capture_rect,
        "frame": {
            "width": result.frame.width,
            "height": result.frame.height,
            "stride": result.frame.stride,
        },
        "window": window,
    })
}

fn cursor_json(cursor: &CursorState, reference_rect: Option<Rect>) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "global": cursor.global,
        "window_relative": cursor.window_relative,
        "inside_reference_rect": cursor.inside_reference_rect,
        "reference_rect": reference_rect,
        "scale": scale_json(cursor.scale),
    }))
}

pub(crate) fn cursor_state_json(cursor: &CursorState) -> Value {
    json!({
        "global": cursor.global,
        "window_relative": cursor.window_relative,
        "inside_reference_rect": cursor.inside_reference_rect,
        "scale": scale_json(cursor.scale),
    })
}

fn debug_bundle_json(
    args: &CaptureArgs,
    result: &CaptureResult,
    cursor: Option<&CursorState>,
    frame_path: &Path,
    metadata_path: &Path,
) -> Value {
    json!({
        "schema_version": 1,
        "config": {
            "match": args.match_title,
            "class": args.class_name,
            "pid": args.pid,
            "exact_title": args.exact_title,
            "manual_region": args.manual_region,
            "backend": args.backend,
            "preset": args.preset,
        },
        "capture": capture_json(result),
        "cursor": cursor.map(cursor_state_json),
        "files": {
            "frame": file_name_only(frame_path),
            "metadata": file_name_only(metadata_path),
        },
    })
}

pub(crate) fn window_candidate_json(candidate: &WindowCandidate) -> Value {
    json!({
        "id": candidate.info.id,
        "id_hex": format!("0x{:x}", candidate.info.id),
        "title": candidate.info.title,
        "class_name": candidate.info.class_name,
        "pid": candidate.pid,
        "active": candidate.is_active,
        "matches_query": candidate.matches_query,
        "rect": candidate.info.rect,
    })
}

/// The scale factor as a JSON number (`1.0`, `1.25`).
pub(crate) fn scale_json(scale: ScaleFactor) -> f64 {
    f64::from(scale.value_milli()) / 1000.0
}

fn relative_or_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn file_name_only(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use d4lc_core::Point;

    #[test]
    fn defaulted_query_uses_diablo_when_no_selector_was_provided() {
        let args = CaptureArgs::default();
        let query = args.defaulted_window_query();
        assert_eq!(query.title.as_deref(), Some("Diablo IV"));
        assert!(!query.exact_title);
    }

    #[test]
    fn explicit_query_does_not_force_diablo_title() {
        let args = CaptureArgs {
            class_name: Some("GeForceNOW".to_string()),
            ..CaptureArgs::default()
        };
        let query = args.defaulted_window_query();
        assert_eq!(query.title, None);
        assert_eq!(query.class_name.as_deref(), Some("GeForceNOW"));
    }

    #[test]
    fn cursor_json_contains_both_coordinate_systems() {
        let cursor = CursorState {
            global: Point { x: 100, y: 200 },
            window_relative: Some(Point { x: 10, y: 20 }),
            inside_reference_rect: Some(true),
            scale: d4lc_capture::ScaleFactor::one(),
        };

        let json = cursor_json(
            &cursor,
            Some(Rect {
                x: 90,
                y: 180,
                width: 500,
                height: 300,
            }),
        );

        let value: Value = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(value["global"]["x"], 100);
        assert_eq!(value["window_relative"]["y"], 20);
        assert_eq!(value["reference_rect"]["width"], 500);
        assert!(json.contains("\"scale\": 1.0"));
    }
}
