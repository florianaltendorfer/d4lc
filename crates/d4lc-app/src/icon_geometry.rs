//! Headless geometry contract for the icon set.
//!
//! The icons are Slint components, so their size contract cannot be asserted
//! by source inspection: this module instantiates the component tree with the
//! Slint testing backend (no display server) and measures the real element
//! tree. It complements the X11 visual smoke; it does not replace it.

#[cfg(test)]
mod tests {
    use i_slint_backend_testing::{ElementHandle, ElementQuery, ElementRoot};
    use slint::ComponentHandle;

    /// Every icon component of `ui/components/icons.slint`.
    const ICON_TYPE_NAMES: &[&str] = &[
        "GithubIcon",
        "CoffeeOutlineIcon",
        "HelpCircleOutlineIcon",
        "TooltipIcon",
        "ScaleBalanceIcon",
        "TooltipTextOutlineIcon",
        "BugIcon",
        "CogIcon",
        "PowerIcon",
        "CameraIcon",
        "RefreshIcon",
        "ExportIcon",
        "TrashIcon",
        "PencilIcon",
        "ExitArrowIcon",
        "InfoOutlineIcon",
        "PlusIcon",
        "CloseIcon",
        "ChevronLeftIcon",
        "ChevronRightIcon",
    ];

    /// The requested drawing-area sizes the shell uses. Every one of them must
    /// render the same glyph, so the contract is checked against the sizes the
    /// screens actually ask for.
    const REQUESTED_SIZES: &[i32] = &[15, 18, 22, 24, 50];

    /// Sub-pixel layout rounding tolerance, in logical pixels.
    const TOLERANCE: f32 = 1.0;

    /// One shown window of the component tree: the icons live in the screen
    /// layout, so their geometry needs a laid-out window.
    fn shown_window() -> crate::slint_generated::AppWindow {
        let app = crate::test_window::new_app_window();
        app.show()
            .unwrap_or_else(|error| panic!("AppWindow::show failed: {error:?}"));
        app
    }

    fn icon_instances(root: &impl ElementRoot) -> Vec<ElementHandle> {
        ElementQuery::from_root(root)
            .find_all()
            .into_iter()
            .filter(|element| {
                element
                    .type_name()
                    .map(|type_name| ICON_TYPE_NAMES.contains(&type_name.as_str()))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// One test body: the Slint testing backend binds its platform to the
    /// thread that installed it, so every window measurement runs here.
    #[test]
    fn icons_hold_their_size_contract_at_every_requested_size() {
        let app = shown_window();
        icon_size_contract(&app);
        debug_pager_fits_a_small_window(&app);
    }

    fn icon_size_contract(app: &crate::slint_generated::AppWindow) {
        let icons = icon_instances(app);
        assert!(
            icons.len() >= 12,
            "the screens must expose their icons to this contract, saw {}",
            icons.len()
        );

        for icon in &icons {
            let type_name = icon.type_name().unwrap_or_default().to_string();
            let position = icon.absolute_position();
            let size = icon.size();
            let requested = size.width.round() as i32;
            assert_eq!(
                requested,
                size.height.round() as i32,
                "{type_name} must be square, saw {}x{}",
                size.width,
                size.height
            );
            assert!(
                REQUESTED_SIZES.contains(&requested),
                "{type_name} is requested at {requested}px, not one of the documented sizes \
                 {REQUESTED_SIZES:?}"
            );

            // Every drawn descendant stays inside the icon's own drawing area:
            // a fixed inner geometry designed for another size overhangs it.
            let mut checked = 0;
            for child in icon.query_descendants().find_all() {
                let child_size = child.size();
                if child_size.width <= 0.0 || child_size.height <= 0.0 {
                    continue;
                }
                let child_position = child.absolute_position();
                let child_right = child_position.x + child_size.width;
                let child_bottom = child_position.y + child_size.height;
                assert!(
                    child_position.x >= position.x - TOLERANCE
                        && child_position.y >= position.y - TOLERANCE
                        && child_right <= position.x + size.width + TOLERANCE
                        && child_bottom <= position.y + size.height + TOLERANCE,
                    "{type_name} at {requested}px: child {:?} at ({}, {}) {}x{} leaves the \
                     drawing area ({}, {}) {}x{}",
                    child.type_name(),
                    child_position.x,
                    child_position.y,
                    child_size.width,
                    child_size.height,
                    position.x,
                    position.y,
                    size.width,
                    size.height,
                );
                checked += 1;
            }
            assert!(
                checked > 0,
                "{type_name} must expose its drawn contour to this contract"
            );
        }

        // The contour a glyph draws with rectangles (the power ring, the
        // help ring, the github plate) must be centered in the requested
        // area; a fixed inner offset for another size shifts it.
        let icons = icon_instances(app);
        for type_name in ["PowerIcon", "HelpCircleOutlineIcon", "GithubIcon"] {
            let icon = icons
                .iter()
                .find(|icon| icon.type_name().as_deref() == Some(type_name))
                .unwrap_or_else(|| panic!("{type_name} not found in the window"));
            let position = icon.absolute_position();
            let size = icon.size();
            let contour = icon
                .query_descendants()
                .find_all()
                .into_iter()
                .filter(|child| child.type_name().as_deref() == Some("Rectangle"))
                .max_by(|left, right| {
                    let left_size = left.size();
                    let right_size = right.size();
                    let left_area = left_size.width * left_size.height;
                    let right_area = right_size.width * right_size.height;
                    left_area
                        .partial_cmp(&right_area)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or_else(|| panic!("{type_name} must draw a rectangular contour"));
            let contour_position = contour.absolute_position();
            let contour_size = contour.size();
            let icon_center = position.x + size.width / 2.0;
            let contour_center = contour_position.x + contour_size.width / 2.0;
            assert!(
                (icon_center - contour_center).abs() <= TOLERANCE,
                "{type_name} contour is not horizontally centered: {contour_center} vs \
                 {icon_center} in a {}px area",
                size.width
            );
        }
    }

    fn element(app: &crate::slint_generated::AppWindow, id: &str) -> ElementHandle {
        let mut found = ElementHandle::find_by_element_id(app, id);
        let first = found
            .next()
            .unwrap_or_else(|| panic!("no element with id {id:?} in the window"));
        assert!(found.next().is_none(), "element id {id:?} is not unique");
        first
    }

    /// Left/right edges of an element in window coordinates.
    fn horizontal_span(element: &ElementHandle) -> (f32, f32) {
        let position = element.absolute_position();
        (position.x, position.x + element.size().width)
    }

    /// The Debug pager stays inside a small window on every panel: the
    /// content never spills over the control column or under a chevron, and
    /// both chevrons stay on screen (upstream's FlipView keeps its arrows
    /// over the item at every window size; its images only scale down,
    /// `DebugView.xaml` `Stretch=Uniform StretchDirection=DownOnly`).
    fn debug_pager_fits_a_small_window(app: &crate::slint_generated::AppWindow) {
        use crate::slint_generated::{
            DebugDiagnosticRow, DebugOcrPanel, DebugOcrRow, DebugStageRow,
        };
        use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

        let long = SharedString::from(
            "a deliberately long provenance line that must elide or wrap instead of widening \
             the pager past the window edge on a small display",
        );
        let stage_image = Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(1920, 1080));
        let stage_rows: Vec<DebugStageRow> = (0..8)
            .map(|index| DebugStageRow {
                label: SharedString::from(format!("Stage {index} {long}")),
                path: long.clone(),
                source: long.clone(),
                note: long.clone(),
                has_image: index % 2 == 0,
                image: stage_image.clone(),
            })
            .collect();
        let ocr_rows: Vec<DebugOcrRow> = (0..3)
            .map(|index| DebugOcrRow {
                id: SharedString::from(format!("row-{index}-{long}")),
                text: long.clone(),
                source: long.clone(),
            })
            .collect();
        let ocr_panels: Vec<DebugOcrPanel> = (0..3)
            .map(|_| DebugOcrPanel {
                title: long.clone(),
                source: long.clone(),
                note: long.clone(),
                columns: ModelRc::new(VecModel::from(vec![
                    long.clone(),
                    long.clone(),
                    long.clone(),
                ])),
                rows: ModelRc::new(VecModel::from(ocr_rows.clone())),
            })
            .collect();
        let diagnostics: Vec<DebugDiagnosticRow> = (0..20)
            .map(|_| DebugDiagnosticRow {
                phase: long.clone(),
                outcome: long.clone(),
                reason: long.clone(),
                generation: i32::MAX,
            })
            .collect();
        app.set_debug_stage_rows(ModelRc::new(VecModel::from(stage_rows)));
        app.set_debug_ocr_panels(ModelRc::new(VecModel::from(ocr_panels)));
        app.set_debug_diagnostics_rows(ModelRc::new(VecModel::from(diagnostics)));
        app.set_runtime_status_text(long.clone());
        app.set_current_item_name(long.clone());
        app.set_active_tab(3);

        for (width, height) in [(1150.0, 650.0), (1000.0, 600.0)] {
            app.window()
                .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
                    width, height,
                )));
            for panel in 0..12 {
                app.set_debug_active_panel_index(panel);
                let (_, column_right) = horizontal_span(&element(app, "DebugScreen::left-column"));
                let (prev_left, prev_right) =
                    horizontal_span(&element(app, "DebugScreen::pager-prev"));
                let (next_left, next_right) =
                    horizontal_span(&element(app, "DebugScreen::pager-next"));
                let content = element(app, "DebugScreen::pager-content");
                let (content_left, content_right) = horizontal_span(&content);
                let context = format!("{width}x{height}, panel {panel}");
                assert!(
                    prev_left >= column_right - TOLERANCE,
                    "{context}: the previous chevron ({prev_left}) sits under the control \
                     column (right edge {column_right})"
                );
                assert!(
                    next_right <= width + TOLERANCE,
                    "{context}: the next chevron ends at {next_right}, past the window edge"
                );
                assert!(
                    content_left >= prev_right - TOLERANCE
                        && content_right <= next_left + TOLERANCE,
                    "{context}: the content slot {content_left}..{content_right} overlaps the \
                     chevrons {prev_right} / {next_left}"
                );
                // Every element of the panel stays inside the slot (the slot clips,
                // so an overflow would be cut off rather than elided).
                if (8..=10).contains(&panel) {
                    ocr_table_is_compact_and_aligned(app, &context);
                }
                let descendants = content.query_descendants().find_all();
                assert!(
                    descendants.len() > 3,
                    "{context}: the active panel must expose its elements to this contract"
                );
                let content_top = content.absolute_position().y;
                let content_bottom = content_top + content.size().height;
                assert!(
                    content_bottom <= height + TOLERANCE,
                    "{context}: the content slot ends at {content_bottom}, below the window"
                );
                for child in descendants {
                    let (left, right) = horizontal_span(&child);
                    if child.size().width <= 0.0 {
                        continue;
                    }
                    let type_name = child.type_name().unwrap_or_default().to_string();
                    assert!(
                        left >= content_left - TOLERANCE && right <= content_right + TOLERANCE,
                        "{context}: {type_name} spans {left}..{right}, outside the content slot \
                         {content_left}..{content_right}"
                    );
                }
            }
        }
    }

    /// The OCR table rows keep their compact height and every row's text
    /// cell starts under the "Text" header (upstream's DataGrid columns).
    fn ocr_table_is_compact_and_aligned(app: &crate::slint_generated::AppWindow, context: &str) {
        let header = element(app, "DebugScreen::ocr-header");
        let mut header_cells: Vec<ElementHandle> = header
            .query_descendants()
            .find_all()
            .into_iter()
            .filter(|cell| cell.type_name().as_deref() == Some("Text"))
            .collect();
        header_cells.sort_by(|left, right| {
            left.absolute_position()
                .x
                .total_cmp(&right.absolute_position().x)
        });
        assert_eq!(header_cells.len(), 3, "{context}: three header cells");
        let text_header_x = header_cells[1].absolute_position().x;

        let rows: Vec<ElementHandle> =
            ElementHandle::find_by_element_id(app, "DebugScreen::ocr-row").collect();
        let texts: Vec<ElementHandle> =
            ElementHandle::find_by_element_id(app, "DebugScreen::ocr-row-text").collect();
        assert!(!rows.is_empty(), "{context}: the OCR table shows its rows");
        for row in &rows {
            let height = row.size().height;
            assert!(
                (height - 24.0).abs() <= TOLERANCE,
                "{context}: an OCR row is {height}px tall, not the compact 24px"
            );
        }
        for text in &texts {
            let x = text.absolute_position().x;
            assert!(
                (x - text_header_x).abs() <= TOLERANCE,
                "{context}: a row's text cell starts at {x}, its header at {text_header_x}"
            );
        }
    }
}
