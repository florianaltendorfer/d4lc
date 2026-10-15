use crate::text::{PayloadCache, TextDiagnostics, TextLineMask, TextRasterizer};
use crate::{
    color_pixel, menu_interaction_zone, overlay_color_pixel, text_wrap_policy, OverlayConfig,
    OverlayDrawTimings, OverlayFrame, OverlayRenderKind, OverlayRenderOp,
};
use d4lc_core::Rect;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::render::{
    Color, ConnectionExt as RenderConnectionExt, CreatePictureAux, PictOp, PictType, Pictformat,
    Picture,
};
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    Arc, AtomEnum, ChangeGCAux, ClipOrdering, ConfigureWindowAux, ConnectionExt, CoordMode,
    CreateGCAux, CreateWindowAux, EventMask, Gcontext, ImageFormat, Point, PolyShape, PropMode,
    Rectangle, VisualClass, Visualid, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

// ARGB-style colours. On non-ARGB visuals the alpha byte is ignored.
const CLEAR_PIXEL: u32 = 0x00000000;
const FG_PIXEL: u32 = 0xFF00FF80;

/// One batched X11 draw request: consecutive render ops with the same
/// kind and foreground color share a single `poly_*` request instead of
/// one request plus two synchronous round trips per op.
/// Batching never reorders draws, so the painted pixels are identical.
#[derive(Debug, Default)]
enum PendingBatch {
    #[default]
    None,
    OutlineRects {
        foreground: u32,
        line_width: u16,
        rects: Vec<Rectangle>,
    },
    FillRects {
        foreground: u32,
        rects: Vec<Rectangle>,
    },
    FillArcs {
        foreground: u32,
        arcs: Vec<Arc>,
    },
    OutlineArcs {
        foreground: u32,
        line_width: u16,
        arcs: Vec<Arc>,
    },
    FillTriangles {
        foreground: u32,
        triangles: Vec<[Point; 3]>,
    },
    OutlineTriangles {
        foreground: u32,
        line_width: u16,
        triangles: Vec<[Point; 3]>,
    },
}

/// The GC state the draw path last latched. Both the foreground and the
/// line width are part of it, so a batch that needs a specific stroke
/// width (1 for an any-type marker rectangle, 2 for a circle/triangle
/// outline) sets exactly that width instead of inheriting whatever the
/// previous batch latched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GcState {
    foreground: Option<u32>,
    line_width: Option<u16>,
}

pub struct RealX11Overlay {
    conn: RustConnection,
    win: Window,
    gc: Gcontext,
    gc_state: GcState,
    overlay_w: u16,
    overlay_h: u16,
    /// The RENDER objects the text and circle paths composite through, or
    /// the exact reason they could not be set up. Without them text is
    /// reported as unavailable (never drawn by replacing the line box) and
    /// circles fall back to the aliased core arcs.
    compositor: Result<RenderCompositor, String>,
    /// The shared bounded text path. `None` when the assigned font is
    /// unavailable: text ops are then skipped and the reason is
    /// reported, never replaced by another face.
    text: Option<TextRasterizer>,
    /// Raster/commit costs of the last completed draw.
    pub(crate) last_draw_timings: OverlayDrawTimings,
    text_mask: TextLineMask,
    text_payloads: PayloadCache,
    text_diagnostics: TextDiagnostics,
    pub always_on_top_requested: bool,
    pub always_on_top_confirmed: bool,
    pub transparent_requested: bool,
    pub transparent_confirmed: bool,
    pub click_through_confirmed: bool,
}

impl RealX11Overlay {
    /// Try to open an X11 connection and create the overlay window.
    /// Returns `Err` if anything essential fails so the caller can fall back.
    pub fn create(config: &OverlayConfig) -> Result<Self, String> {
        let (conn, screen_num) =
            RustConnection::connect(None).map_err(|e| format!("x11rb connect: {e}"))?;

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;

        let overlay_x = config
            .window_geometry
            .x
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let overlay_y = config
            .window_geometry
            .y
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let overlay_w = config.window_geometry.width.min(u16::MAX as u32) as u16;
        let overlay_h = config.window_geometry.height.min(u16::MAX as u32) as u16;

        // Attempt to find a 32-bit ARGB visual for transparency.
        // If not found we fall back to the screen's default visual.
        // Finding an ARGB visual only means transparency was requested; it
        // does not prove compositor-supported transparency is actually visible.
        let (visual_id, depth, transparent_requested) = find_argb_visual(&conn, screen_num)
            .unwrap_or((screen.root_visual, screen.root_depth, false));

        // The shared bounded text path (explicit font assignment, font
        // size, text area and wrapping). A failure is reported, not
        // worked around with another font.
        let mut text_diagnostics = TextDiagnostics::default();
        let text = match TextRasterizer::load_system() {
            Ok(rasterizer) => Some(rasterizer),
            Err(error) => {
                text_diagnostics.record_font_unavailable(&error);
                eprintln!("d4lc-overlay-x11: overlay text disabled: {error}");
                None
            }
        };
        // We need a matching colormap when using a non-default visual.
        let colormap = if visual_id != screen.root_visual {
            let cm = conn
                .generate_id()
                .map_err(|e| format!("generate_id colormap: {e}"))?;
            conn.create_colormap(
                x11rb::protocol::xproto::ColormapAlloc::NONE,
                cm,
                root,
                visual_id,
            )
            .map_err(|e| format!("create_colormap: {e}"))?
            .check()
            .map_err(|e| format!("create_colormap check: {e}"))?;
            Some(cm)
        } else {
            None
        };

        let win = conn
            .generate_id()
            .map_err(|e| format!("generate_id window: {e}"))?;

        let mut aux = CreateWindowAux::new()
            .background_pixel(CLEAR_PIXEL)
            .border_pixel(0)
            .override_redirect(1u32)
            .event_mask(EventMask::EXPOSURE);

        if let Some(cm) = colormap {
            aux = aux.colormap(cm);
        }

        conn.create_window(
            depth,
            win,
            root,
            overlay_x,
            overlay_y,
            overlay_w,
            overlay_h,
            0,
            WindowClass::INPUT_OUTPUT,
            visual_id,
            &aux,
        )
        .map_err(|e| format!("create_window: {e}"))?
        .check()
        .map_err(|e| format!("create_window check: {e}"))?;

        // WM_NAME so it's identifiable.
        WrapperConnectionExt::change_property8(
            &conn,
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"d4lc overlay",
        )
        .map_err(|e| format!("change_property WM_NAME: {e}"))?
        .check()
        .map_err(|e| format!("change_property WM_NAME check: {e}"))?;

        // EWMH always-on-top. With override_redirect the WM may ignore EWMH
        // state; this is only a request, not a confirmed capability.
        let always_on_top_requested = try_set_always_on_top(&conn, win, root);

        let click_through_confirmed = apply_click_through_input_shape(
            &conn,
            win,
            menu_interaction_zone(
                config.window_geometry,
                config.overlay_icon_pos_x,
                config.overlay_icon_pos_y,
            ),
        );

        // Map (show) the window.
        conn.map_window(win)
            .map_err(|e| format!("map_window: {e}"))?
            .check()
            .map_err(|e| format!("map_window check: {e}"))?;

        // Create a GC for drawing.
        let gc = conn
            .generate_id()
            .map_err(|e| format!("generate_id gc: {e}"))?;
        let gc_aux = CreateGCAux::new()
            .foreground(FG_PIXEL)
            .background(CLEAR_PIXEL);
        conn.create_gc(gc, win, &gc_aux)
            .map_err(|e| format!("create_gc: {e}"))?
            .check()
            .map_err(|e| format!("create_gc check: {e}"))?;

        let compositor = RenderCompositor::create(&conn, win, visual_id);
        if let Err(reason) = &compositor {
            eprintln!("d4lc-overlay-x11: overlay text disabled: {reason}");
        }

        conn.flush().map_err(|e| format!("flush: {e}"))?;

        Ok(Self {
            conn,
            win,
            gc,
            gc_state: GcState {
                foreground: Some(FG_PIXEL),
                line_width: Some(1),
            },
            overlay_w,
            overlay_h,
            compositor,
            text,
            text_mask: TextLineMask::default(),
            text_payloads: PayloadCache::default(),
            text_diagnostics,
            always_on_top_requested,
            always_on_top_confirmed: false,
            transparent_requested,
            transparent_confirmed: false,
            click_through_confirmed,
            last_draw_timings: OverlayDrawTimings::default(),
        })
    }

    /// Follow the game window: move and resize the overlay window and
    /// re-apply the click-through input shape for the new menu zone.
    pub fn reposition(&mut self, geometry: Rect, interaction_zone: Rect) -> Result<(), String> {
        let width = geometry.width.clamp(1, u32::from(u16::MAX));
        let height = geometry.height.clamp(1, u32::from(u16::MAX));
        let aux = ConfigureWindowAux::new()
            .x(geometry.x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)))
            .y(geometry.y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)))
            .width(width)
            .height(height);
        self.conn
            .configure_window(self.win, &aux)
            .map_err(|e| format!("configure_window: {e}"))?
            .check()
            .map_err(|e| format!("configure_window check: {e}"))?;
        self.overlay_w = width as u16;
        self.overlay_h = height as u16;
        self.click_through_confirmed =
            apply_click_through_input_shape(&self.conn, self.win, interaction_zone);
        self.conn.flush().map_err(|e| format!("flush: {e}"))?;
        Ok(())
    }

    pub fn draw(
        &mut self,
        _frame: &OverlayFrame,
        render_ops: &[OverlayRenderOp],
    ) -> Result<(), String> {
        let draw_start = Instant::now();
        // Drain pending events (expose etc.) without blocking.
        self.drain_events();

        // Reset to the transparent background. The clear GC foreground is
        // tracked below so no redundant `change_gc` is sent for it.
        let bg_rect = Rectangle {
            x: 0,
            y: 0,
            width: self.overlay_w,
            height: self.overlay_h,
        };
        let mut gc_state = self.gc_state;
        self.set_gc_state(&mut gc_state, CLEAR_PIXEL, 1)?;
        self.conn
            .poly_fill_rectangle(self.win, self.gc, &[bg_rect])
            .map_err(|e| format!("poly_fill_rectangle: {e}"))?;

        // (overlay update budget): the live loop calls
        // `update` synchronously once per frame, so this draw path
        // batches consecutive same-kind/same-color render ops into one
        // `poly_*` request and skips redundant `change_gc` requests
        // instead of paying a `change_gc` + draw request with two
        // synchronous `.check()` round trips per render op. Parity notes:
        //
        // - render ops are still flushed strictly in z-order; only
        //   *consecutive* ops with identical kind, foreground color and
        //   stroke width are merged, so the painted pixels are identical;
        // - the GC state (foreground and line width) is tracked
        //   explicitly, so a 1px marker outline is never drawn with a
        //   previously latched 2px stroke;
        // - protocol errors are still detected and still fail the overlay
        //   (backend `Failed`, drawing disabled) — via one `check_error`
        //   sync after the flush instead of per op, so the error string no
        //   longer names the op index.
        let mut pending = PendingBatch::None;

        for (i, operation) in render_ops.iter().enumerate() {
            let foreground = overlay_color_pixel(&operation.color);
            match operation.kind {
                OverlayRenderKind::OutlineRect | OverlayRenderKind::StrokedRect { .. } => {
                    let line_width = match operation.kind {
                        OverlayRenderKind::StrokedRect { stroke } => {
                            stroke.clamp(1, u32::from(u16::MAX)) as u16
                        }
                        _ => 1,
                    };
                    let rect = rect_to_x11_rectangle(operation.geometry);
                    let continues_batch = matches!(
                        &pending,
                        PendingBatch::OutlineRects {
                            foreground: batch_foreground,
                            line_width: batch_width,
                            ..
                        } if *batch_foreground == foreground && *batch_width == line_width
                    );
                    if !continues_batch {
                        self.flush_batch(&mut pending, &mut gc_state)?;
                        pending = PendingBatch::OutlineRects {
                            foreground,
                            line_width,
                            rects: Vec::new(),
                        };
                    }
                    if let PendingBatch::OutlineRects { rects, .. } = &mut pending {
                        rects.push(rect);
                    }
                }
                OverlayRenderKind::FilledRect => {
                    let rect = rect_to_x11_rectangle(operation.geometry);
                    let continues_batch = matches!(
                        &pending,
                        PendingBatch::FillRects {
                            foreground: batch_foreground,
                            ..
                        } if *batch_foreground == foreground
                    );
                    if !continues_batch {
                        self.flush_batch(&mut pending, &mut gc_state)?;
                        pending = PendingBatch::FillRects {
                            foreground,
                            rects: Vec::new(),
                        };
                    }
                    if let PendingBatch::FillRects { rects, .. } = &mut pending {
                        rects.push(rect);
                    }
                }
                OverlayRenderKind::OutlineFillRect { stroke } => {
                    // Upstream `OutlineFillRectangle(black, affixColor,
                    // ..., stroke)`: fill the rectangle with the marker
                    // color, then stroke the full rectangle in black
                    // with the marker's stroke width. The fill batch is
                    // flushed before the stroke so per-rectangle
                    // fill/stroke order (and cross-marker z-order) is
                    // preserved.
                    let rect = rect_to_x11_rectangle(operation.geometry);
                    let continues_batch = matches!(
                        &pending,
                        PendingBatch::FillRects {
                            foreground: batch_foreground,
                            ..
                        } if *batch_foreground == foreground
                    );
                    if !continues_batch {
                        self.flush_batch(&mut pending, &mut gc_state)?;
                        pending = PendingBatch::FillRects {
                            foreground,
                            rects: Vec::new(),
                        };
                    }
                    if let PendingBatch::FillRects { rects, .. } = &mut pending {
                        rects.push(rect);
                    }
                    self.flush_batch(&mut pending, &mut gc_state)?;
                    pending = PendingBatch::OutlineRects {
                        foreground: color_pixel("Black"),
                        line_width: u16::from(stroke),
                        rects: vec![rect],
                    };
                }
                OverlayRenderKind::Circle if self.compositor.is_ok() => {
                    // Upstream `OutlineFillCircle(_brushes[Colors.Black],
                    // _brushes[affixColor], x, y, radius, 2)`, antialiased
                    // like upstream's Direct2D: the shared shape raster
                    // composited OVER the window in z-order.
                    self.flush_batch(&mut pending, &mut gc_state)?;
                    self.draw_circle_op(operation, i)?;
                }
                OverlayRenderKind::Circle => {
                    // Without RENDER: fill the circle with the marker color,
                    // then stroke the full arc in black with line width 2
                    // (aliased). The fill batch is flushed before the stroke
                    // so per-circle fill/stroke order (and cross-circle
                    // z-order) is preserved.
                    let arc = arc_from_rect(operation.geometry);
                    let continues_batch = matches!(
                        &pending,
                        PendingBatch::FillArcs {
                            foreground: batch_foreground,
                            ..
                        } if *batch_foreground == foreground
                    );
                    if !continues_batch {
                        self.flush_batch(&mut pending, &mut gc_state)?;
                        pending = PendingBatch::FillArcs {
                            foreground,
                            arcs: Vec::new(),
                        };
                    }
                    if let PendingBatch::FillArcs { arcs, .. } = &mut pending {
                        arcs.push(arc);
                    }
                    self.flush_batch(&mut pending, &mut gc_state)?;
                    pending = PendingBatch::OutlineArcs {
                        foreground: color_pixel("Black"),
                        line_width: 2,
                        arcs: vec![arc],
                    };
                }
                OverlayRenderKind::Triangle => {
                    // Upstream `FillTriangle(affixColor)` +
                    // `DrawTriangle(Colors.Black, 2)` for greater
                    // affixes (`OverlayHandler.cs` 305-310): fill the
                    // triangle with the marker color, then stroke the
                    // closed triangle in black with line width 2.
                    // The fill batch is flushed before the stroke so
                    // per-triangle fill/stroke order (and cross-marker
                    // z-order) is preserved.
                    let triangle = triangle_points_from_rect(operation.geometry);
                    let continues_batch = matches!(
                        &pending,
                        PendingBatch::FillTriangles {
                            foreground: batch_foreground,
                            ..
                        } if *batch_foreground == foreground
                    );
                    if !continues_batch {
                        self.flush_batch(&mut pending, &mut gc_state)?;
                        pending = PendingBatch::FillTriangles {
                            foreground,
                            triangles: Vec::new(),
                        };
                    }
                    if let PendingBatch::FillTriangles { triangles, .. } = &mut pending {
                        triangles.push(triangle);
                    }
                    self.flush_batch(&mut pending, &mut gc_state)?;
                    pending = PendingBatch::OutlineTriangles {
                        foreground: color_pixel("Black"),
                        line_width: 2,
                        triangles: vec![triangle],
                    };
                }
                OverlayRenderKind::Text => {
                    // The text payload carries its own color (one
                    // `put_image` per laid-out line), so no `change_gc`
                    // is needed for it; the pending batch is still
                    // flushed first to keep strict z-order.
                    self.flush_batch(&mut pending, &mut gc_state)?;
                    self.draw_text_op(operation, i)?;
                }
            }
        }
        self.flush_batch(&mut pending, &mut gc_state)?;
        self.gc_state = gc_state;

        let raster_us = OverlayDrawTimings::micros(draw_start.elapsed());
        self.conn.flush().map_err(|e| format!("flush: {e}"))?;
        // One sync round trip: the X server processes every request of
        // this frame before answering, and any protocol error it
        // generated for them is queued as an event ahead of the sync
        // reply. Drain those events so a broken draw still fails the
        // overlay exactly like the per-op `.check()` implementation did.
        let _focus = self
            .conn
            .get_input_focus()
            .map_err(|e| format!("overlay draw sync: {e}"))?
            .reply()
            .map_err(|e| format!("overlay draw sync reply: {e}"))?;
        while let Some(event) = self
            .conn
            .poll_for_event()
            .map_err(|e| format!("overlay draw poll: {e}"))?
        {
            if let Event::Error(error) = event {
                return Err(format!("overlay draw protocol error: {error:?}"));
            }
        }
        self.last_draw_timings = OverlayDrawTimings {
            raster_us: Some(raster_us),
            commit_us: Some(
                OverlayDrawTimings::micros(draw_start.elapsed()).saturating_sub(raster_us),
            ),
        };
        Ok(())
    }

    /// Draws one text render op through the shared bounded text path:
    /// layout inside the op's area under the op's wrap contract, then
    /// composite the op's colour OVER the window through each line's glyph
    /// mask (uploaded as an A8 picture) — the same coverage and alpha the
    /// Wayland backend blends, so identical render ops produce identical
    /// text pixels on both, and the panel beneath shows between the glyphs.
    ///
    /// An unavailable font or a missing RENDER setup skips the op and
    /// reports the reason through the text limitation; it is never worked
    /// around with a substitute face or an opaque line box.
    fn draw_text_op(&mut self, operation: &OverlayRenderOp, index: usize) -> Result<(), String> {
        let Some(text) = operation.text.as_deref() else {
            return Ok(());
        };
        let Ok(compositor) = &self.compositor else {
            return Ok(());
        };
        let rasterizer = match self.text.as_mut() {
            Some(rasterizer) => rasterizer,
            None => return Ok(()),
        };
        let color = overlay_color_pixel(&operation.color);
        let opacity = (color >> 24) as u8;
        let layout = rasterizer.layout(
            operation.geometry,
            text_wrap_policy(operation),
            text,
            operation.font_px,
        );
        self.text_diagnostics.record(&layout);
        if layout.lines.is_empty() {
            return Ok(());
        }

        // The colour itself is opaque; the op's opacity rides in the mask
        // with the glyph coverage, exactly like `text_pixel_argb`.
        let channel = |shift: u32| (((color >> shift) & 0xFF) as u16) * 0x101;
        let source = self
            .conn
            .generate_id()
            .map_err(|e| format!("generate_id text source {index}: {e}"))?;
        self.conn
            .render_create_solid_fill(
                source,
                Color {
                    red: channel(16),
                    green: channel(8),
                    blue: channel(0),
                    alpha: 0xFFFF,
                },
            )
            .map_err(|e| format!("create_solid_fill descriptor {index}: {e}"))?;

        for line in &layout.lines {
            rasterizer.rasterize_line_into(line, &mut self.text_mask);
            if self.text_mask.coverage.is_empty() {
                continue;
            }
            let width = self.text_mask.width.min(u16::MAX as u32) as u16;
            let height = self.text_mask.height.min(u16::MAX as u32) as u16;
            let payload = self.text_payloads.payload(
                &self.text_mask,
                opacity,
                compositor.alpha_row_stride(u32::from(width)),
            );
            let pixmap = self
                .conn
                .generate_id()
                .map_err(|e| format!("generate_id text mask {index}: {e}"))?;
            self.conn
                .create_pixmap(8, pixmap, self.win, width, height)
                .map_err(|e| format!("create_pixmap descriptor {index}: {e}"))?;
            self.conn
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    pixmap,
                    compositor.alpha_gc,
                    width,
                    height,
                    0,
                    0,
                    0,
                    8,
                    &payload,
                )
                .map_err(|e| format!("put_image descriptor {index}: {e}"))?;
            let mask = self
                .conn
                .generate_id()
                .map_err(|e| format!("generate_id text picture {index}: {e}"))?;
            self.conn
                .render_create_picture(
                    mask,
                    pixmap,
                    compositor.alpha_format,
                    &CreatePictureAux::new(),
                )
                .map_err(|e| format!("create_picture descriptor {index}: {e}"))?;
            self.conn
                .render_composite(
                    PictOp::OVER,
                    source,
                    mask,
                    compositor.window_picture,
                    0,
                    0,
                    0,
                    0,
                    to_i16_coord(self.text_mask.x),
                    to_i16_coord(self.text_mask.y),
                    width,
                    height,
                )
                .map_err(|e| format!("composite descriptor {index}: {e}"))?;
            self.conn
                .render_free_picture(mask)
                .map_err(|e| format!("free_picture descriptor {index}: {e}"))?;
            self.conn
                .free_pixmap(pixmap)
                .map_err(|e| format!("free_pixmap descriptor {index}: {e}"))?;
        }
        self.conn
            .render_free_picture(source)
            .map_err(|e| format!("free_picture text source {index}: {e}"))?;
        Ok(())
    }

    /// Lays the overlay text out and reports what the assignment could not
    /// draw (unsupported codepoints, clipped characters, unavailable
    /// font). Never mutates: a backend diagnostic read.
    pub fn text_diagnostics(&self) -> &TextDiagnostics {
        &self.text_diagnostics
    }

    /// Draws one circle render op: the shared antialiased raster
    /// ([`crate::shape::outline_fill_circle`]) uploaded as an ARGB32 picture
    /// and composited OVER the window, so the edge blends into what lies
    /// beneath exactly like the Wayland canvas blend.
    fn draw_circle_op(&mut self, operation: &OverlayRenderOp, index: usize) -> Result<(), String> {
        let Ok(compositor) = &self.compositor else {
            return Ok(());
        };
        let color = overlay_color_pixel(&operation.color);
        let fill = crate::text::text_pixel_argb(color, (color >> 24) as u8, 255);
        let Some(image) = crate::shape::outline_fill_circle(operation.geometry, fill) else {
            return Ok(());
        };
        let width = image.width.min(u16::MAX as u32) as u16;
        let height = image.height.min(u16::MAX as u32) as u16;
        let pixmap = self
            .conn
            .generate_id()
            .map_err(|e| format!("generate_id circle pixmap {index}: {e}"))?;
        self.conn
            .create_pixmap(32, pixmap, self.win, width, height)
            .map_err(|e| format!("create_pixmap circle {index}: {e}"))?;
        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                compositor.argb_gc,
                width,
                height,
                0,
                0,
                0,
                32,
                &image.to_le_bytes(),
            )
            .map_err(|e| format!("put_image circle {index}: {e}"))?;
        let picture = self
            .conn
            .generate_id()
            .map_err(|e| format!("generate_id circle picture {index}: {e}"))?;
        self.conn
            .render_create_picture(
                picture,
                pixmap,
                compositor.argb_format,
                &CreatePictureAux::new(),
            )
            .map_err(|e| format!("create_picture circle {index}: {e}"))?;
        self.conn
            .render_composite(
                PictOp::OVER,
                picture,
                x11rb::NONE,
                compositor.window_picture,
                0,
                0,
                0,
                0,
                to_i16_coord(image.x),
                to_i16_coord(image.y),
                width,
                height,
            )
            .map_err(|e| format!("composite circle {index}: {e}"))?;
        self.conn
            .render_free_picture(picture)
            .map_err(|e| format!("free_picture circle {index}: {e}"))?;
        self.conn
            .free_pixmap(pixmap)
            .map_err(|e| format!("free_pixmap circle {index}: {e}"))?;
        Ok(())
    }

    /// One limitation line for the status: the resolved font
    /// assignment and its contract, or the exact reason text is disabled.
    pub fn text_limitation(&self) -> String {
        if let Some(font_unavailable) = &self.text_diagnostics.font_unavailable {
            return format!("overlay text disabled: {font_unavailable}");
        }
        if let Err(reason) = &self.compositor {
            return format!("overlay text disabled: {reason}");
        }
        match &self.text {
            Some(rasterizer) => format!(
                "overlay text: {} at OverlayFontSize, wrapped panels, single-line markers \
                 and exact paragon lines (Consolas Bold, else bundled Inconsolata Bold)",
                rasterizer.assignment()
            ),
            None => "overlay text disabled: no font assignment".to_string(),
        }
    }

    /// Sends the GC state only when it actually changes (foreground or
    /// line width).
    fn set_gc_state(
        &self,
        gc_state: &mut GcState,
        foreground: u32,
        line_width: u16,
    ) -> Result<(), String> {
        let wanted = GcState {
            foreground: Some(foreground),
            line_width: Some(line_width),
        };
        if *gc_state == wanted {
            return Ok(());
        }
        let gc_aux = ChangeGCAux::new()
            .foreground(foreground)
            .background(CLEAR_PIXEL)
            .line_width(u32::from(line_width));
        self.conn
            .change_gc(self.gc, &gc_aux)
            .map_err(|e| format!("change_gc: {e}"))?;
        *gc_state = wanted;
        Ok(())
    }

    /// Submit the pending batch as a single X11 request with the right GC
    /// state (foreground and stroke width), leaving `pending` as `None`.
    fn flush_batch(
        &self,
        pending: &mut PendingBatch,
        gc_state: &mut GcState,
    ) -> Result<(), String> {
        match std::mem::take(pending) {
            PendingBatch::None => Ok(()),
            PendingBatch::OutlineRects {
                foreground,
                line_width,
                rects,
            } => {
                if rects.is_empty() {
                    return Ok(());
                }
                self.set_gc_state(gc_state, foreground, line_width)?;
                self.conn
                    .poly_rectangle(self.win, self.gc, &rects)
                    .map(|_void| ())
                    .map_err(|e| format!("poly_rectangle: {e}"))
            }
            PendingBatch::FillRects { foreground, rects } => {
                if rects.is_empty() {
                    return Ok(());
                }
                // `Fill` ignores the stroke width, so the current width is
                // kept as-is instead of forcing a `change_gc` per batch.
                self.set_gc_state(gc_state, foreground, gc_state.line_width.unwrap_or(1))?;
                self.conn
                    .poly_fill_rectangle(self.win, self.gc, &rects)
                    .map(|_void| ())
                    .map_err(|e| format!("poly_fill_rectangle: {e}"))
            }
            PendingBatch::FillArcs { foreground, arcs } => {
                if arcs.is_empty() {
                    return Ok(());
                }
                self.set_gc_state(gc_state, foreground, gc_state.line_width.unwrap_or(1))?;
                self.conn
                    .poly_fill_arc(self.win, self.gc, &arcs)
                    .map(|_void| ())
                    .map_err(|e| format!("poly_fill_arc: {e}"))
            }
            PendingBatch::OutlineArcs {
                foreground,
                line_width,
                arcs,
            } => {
                if arcs.is_empty() {
                    return Ok(());
                }
                self.set_gc_state(gc_state, foreground, line_width)?;
                self.conn
                    .poly_arc(self.win, self.gc, &arcs)
                    .map(|_void| ())
                    .map_err(|e| format!("poly_arc: {e}"))
            }
            PendingBatch::FillTriangles {
                foreground,
                triangles,
            } => {
                if triangles.is_empty() {
                    return Ok(());
                }
                self.set_gc_state(gc_state, foreground, gc_state.line_width.unwrap_or(1))?;
                // `FillPoly` fills a single polygon per request, so a
                // batch submits one request per triangle while sharing
                // the GC foreground state.
                for triangle in &triangles {
                    self.conn
                        .fill_poly(
                            self.win,
                            self.gc,
                            PolyShape::CONVEX,
                            CoordMode::ORIGIN,
                            triangle,
                        )
                        .map(|_void| ())
                        .map_err(|e| format!("fill_poly: {e}"))?;
                }
                Ok(())
            }
            PendingBatch::OutlineTriangles {
                foreground,
                line_width,
                triangles,
            } => {
                if triangles.is_empty() {
                    return Ok(());
                }
                self.set_gc_state(gc_state, foreground, line_width)?;
                for triangle in &triangles {
                    // Close the outline: repeat the apex as the last point
                    // so `DrawTriangle`'s three edges are all stroked.
                    let closed = [triangle[0], triangle[1], triangle[2], triangle[0]];
                    self.conn
                        .poly_line(CoordMode::ORIGIN, self.win, self.gc, &closed)
                        .map(|_void| ())
                        .map_err(|e| format!("poly_line: {e}"))?;
                }
                Ok(())
            }
        }
    }

    /// Drain pending events (expose etc.) without blocking. Returns
    /// whether an Expose event was seen: the server lost pixels, so the
    /// caller must repaint.
    pub(crate) fn drain_events(&self) -> bool {
        let mut expose_seen = false;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if matches!(event, Event::Expose(_)) {
                expose_seen = true;
            }
        }
        expose_seen
    }

    #[cfg(test)]
    pub fn capture_window_pixels_for_test(&self) -> Result<(u32, u32, usize, Vec<u8>), String> {
        let reply = self
            .conn
            .get_image(
                x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
                self.win,
                0,
                0,
                self.overlay_w,
                self.overlay_h,
                u32::MAX,
            )
            .map_err(|error| format!("get_image: {error}"))?
            .reply()
            .map_err(|error| format!("get_image reply: {error}"))?;

        let width = usize::from(self.overlay_w);
        let height = usize::from(self.overlay_h);
        let pixel_count = width.saturating_mul(height);
        if pixel_count == 0 {
            return Err("captured overlay window has zero pixels".to_string());
        }

        let bytes_per_pixel = reply.data.len() / pixel_count;
        if bytes_per_pixel == 0 || bytes_per_pixel.saturating_mul(pixel_count) != reply.data.len() {
            return Err(format!(
                "unsupported captured image layout: {} bytes for {width}x{height}",
                reply.data.len()
            ));
        }

        Ok((
            u32::from(self.overlay_w),
            u32::from(self.overlay_h),
            bytes_per_pixel,
            reply.data,
        ))
    }
}

impl Drop for RealX11Overlay {
    fn drop(&mut self) {
        if let Ok(compositor) = &self.compositor {
            let _ = self.conn.render_free_picture(compositor.window_picture);
            let _ = self.conn.free_gc(compositor.alpha_gc);
            let _ = self.conn.free_gc(compositor.argb_gc);
        }
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}

/// The RENDER objects the text and circle paths need: a picture on the
/// overlay window, the standard A8 format the glyph masks are uploaded in and
/// the standard ARGB32 format the circle rasters are uploaded in, a GC for
/// each pixmap depth and the server's scanline padding for 8-bit rows.
struct RenderCompositor {
    window_picture: Picture,
    alpha_format: Pictformat,
    alpha_gc: Gcontext,
    argb_format: Pictformat,
    argb_gc: Gcontext,
    /// Bytes each A8 row is padded to (the depth-8 `scanline_pad`).
    alpha_row_pad: u32,
}

impl RenderCompositor {
    /// Sets the objects up on `win`, or names the missing piece.
    fn create(conn: &RustConnection, win: Window, visual: Visualid) -> Result<Self, String> {
        conn.render_query_version(0, 11)
            .map_err(|e| format!("RENDER query_version: {e}"))?
            .reply()
            .map_err(|e| format!("the X server has no RENDER extension: {e}"))?;
        let formats = conn
            .render_query_pict_formats()
            .map_err(|e| format!("RENDER query_pict_formats: {e}"))?
            .reply()
            .map_err(|e| format!("RENDER query_pict_formats reply: {e}"))?;
        let window_format = formats
            .screens
            .iter()
            .flat_map(|screen| &screen.depths)
            .flat_map(|depth| &depth.visuals)
            .find(|candidate| candidate.visual == visual)
            .map(|candidate| candidate.format)
            .ok_or_else(|| format!("RENDER has no picture format for visual {visual:#x}"))?;
        let alpha_format = formats
            .formats
            .iter()
            .find(|format| {
                format.type_ == PictType::DIRECT
                    && format.depth == 8
                    && format.direct.alpha_mask == 0xFF
                    && format.direct.red_mask == 0
                    && format.direct.green_mask == 0
                    && format.direct.blue_mask == 0
            })
            .map(|format| format.id)
            .ok_or_else(|| "RENDER has no A8 picture format".to_string())?;
        let argb_format = formats
            .formats
            .iter()
            .find(|format| {
                format.type_ == PictType::DIRECT
                    && format.depth == 32
                    && format.direct.alpha_shift == 24
                    && format.direct.alpha_mask == 0xFF
                    && format.direct.red_shift == 16
                    && format.direct.red_mask == 0xFF
                    && format.direct.green_shift == 8
                    && format.direct.green_mask == 0xFF
                    && format.direct.blue_shift == 0
                    && format.direct.blue_mask == 0xFF
            })
            .map(|format| format.id)
            .ok_or_else(|| "RENDER has no ARGB32 picture format".to_string())?;
        let alpha_row_pad = conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|format| format.depth == 8 && format.bits_per_pixel == 8)
            .map(|format| u32::from(format.scanline_pad / 8).max(1))
            .ok_or_else(|| "the X server has no 8-bit pixmap format".to_string())?;

        let window_picture = conn
            .generate_id()
            .map_err(|e| format!("generate_id window picture: {e}"))?;
        conn.render_create_picture(window_picture, win, window_format, &CreatePictureAux::new())
            .map_err(|e| format!("create_picture window: {e}"))?
            .check()
            .map_err(|e| format!("create_picture window check: {e}"))?;

        let alpha_gc = pixmap_gc(conn, win, 8)?;
        let argb_gc = pixmap_gc(conn, win, 32)?;

        Ok(Self {
            window_picture,
            alpha_format,
            alpha_gc,
            argb_format,
            argb_gc,
            alpha_row_pad,
        })
    }

    /// The padded byte length of one A8 row of `width` pixels.
    fn alpha_row_stride(&self, width: u32) -> usize {
        width.div_ceil(self.alpha_row_pad) as usize * self.alpha_row_pad as usize
    }
}

/// A GC for pixmaps of `depth`. A GC serves every drawable of the depth and
/// screen it was created for, so one made on a scratch pixmap uploads all
/// images of that depth.
fn pixmap_gc(conn: &RustConnection, win: Window, depth: u8) -> Result<Gcontext, String> {
    let scratch = conn
        .generate_id()
        .map_err(|e| format!("generate_id scratch pixmap: {e}"))?;
    conn.create_pixmap(depth, scratch, win, 1, 1)
        .map_err(|e| format!("create_pixmap depth {depth}: {e}"))?
        .check()
        .map_err(|e| format!("create_pixmap depth {depth} check: {e}"))?;
    let gc = conn
        .generate_id()
        .map_err(|e| format!("generate_id depth {depth} gc: {e}"))?;
    let created = conn
        .create_gc(gc, scratch, &CreateGCAux::new())
        .map_err(|e| format!("create_gc depth {depth}: {e}"))
        .and_then(|cookie| {
            cookie
                .check()
                .map_err(|e| format!("create_gc depth {depth} check: {e}"))
        });
    let _ = conn.free_pixmap(scratch);
    created.map(|()| gc)
}

/// Try to find a 32-bit ARGB visual. Returns (visual_id, depth, true) on
/// success, or None if no ARGB visual is available.
fn find_argb_visual(conn: &RustConnection, screen_num: usize) -> Option<(u32, u8, bool)> {
    let screen = &conn.setup().roots[screen_num];
    for depth_info in &screen.allowed_depths {
        if depth_info.depth != 32 {
            continue;
        }
        for visual in &depth_info.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some((visual.visual_id, 32, true));
            }
        }
    }
    None
}

/// Attempt to set `_NET_WM_STATE_ABOVE` on the window.
/// Returns whether the hint was sent successfully.
fn try_set_always_on_top(conn: &RustConnection, win: Window, _root: Window) -> bool {
    let net_wm_state = match conn.intern_atom(false, b"_NET_WM_STATE") {
        Ok(cookie) => match cookie.reply() {
            Ok(reply) => reply.atom,
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    let net_wm_state_above = match conn.intern_atom(false, b"_NET_WM_STATE_ABOVE") {
        Ok(cookie) => match cookie.reply() {
            Ok(reply) => reply.atom,
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    if net_wm_state == x11rb::NONE || net_wm_state_above == x11rb::NONE {
        return false;
    }

    // Set the property before mapping (works even without a running WM).
    use x11rb::wrapper::ConnectionExt as WrapperExt;
    let result = WrapperExt::change_property32(
        conn,
        PropMode::REPLACE,
        win,
        net_wm_state,
        AtomEnum::ATOM,
        &[net_wm_state_above],
    );

    result.map(|cookie| cookie.check().is_ok()).unwrap_or(false)
}

fn apply_click_through_input_shape(
    conn: &RustConnection,
    win: Window,
    interaction_zone: Rect,
) -> bool {
    let shape_supported = match conn.shape_query_version() {
        Ok(cookie) => cookie.reply().is_ok(),
        Err(_) => false,
    };
    if !shape_supported {
        return false;
    }

    let rect = rect_to_x11_rectangle(interaction_zone);
    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[rect],
    )
    .map(|cookie| cookie.check().is_ok())
    .unwrap_or(false)
}

fn rect_to_x11_rectangle(rect: Rect) -> x11rb::protocol::xproto::Rectangle {
    x11rb::protocol::xproto::Rectangle {
        x: rect.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        y: rect.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        width: rect.width.min(u16::MAX as u32) as u16,
        height: rect.height.min(u16::MAX as u32) as u16,
    }
}

fn arc_from_rect(rect: Rect) -> x11rb::protocol::xproto::Arc {
    // Full circle from the bounding square: start at 3 o'clock, sweep
    // 360 degrees (X11 angles are in 64ths of a degree).
    x11rb::protocol::xproto::Arc {
        x: rect.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        y: rect.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        width: rect.width.min(u16::MAX as u32) as u16,
        height: rect.height.min(u16::MAX as u32) as u16,
        angle1: 0,
        angle2: 360 * 64,
    }
}

fn to_i16_coord(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Triangle vertices from the marker's bounding box: apex at the
/// top-center, base on the bottom edge — upstream `Triangle(left -
/// length / 2, top + length, left + length / 2, top + length, left, top)`
/// (`OverlayHandler.cs` 307).
fn triangle_points_from_rect(rect: Rect) -> [x11rb::protocol::xproto::Point; 3] {
    let apex_x = rect.x + rect.width as i32 / 2;
    let base_y = rect.y.saturating_add(rect.height as i32);
    [
        x11rb::protocol::xproto::Point {
            x: to_i16_coord(rect.x),
            y: to_i16_coord(base_y),
        },
        x11rb::protocol::xproto::Point {
            x: to_i16_coord(rect.x.saturating_add(rect.width as i32)),
            y: to_i16_coord(base_y),
        },
        x11rb::protocol::xproto::Point {
            x: to_i16_coord(apex_x),
            y: to_i16_coord(rect.y),
        },
    ]
}

#[cfg(test)]
mod tests;
