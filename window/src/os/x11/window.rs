use super::*;
use crate::bitmaps::*;
use crate::connection::ConnectionOps;
use crate::os::{xkeysyms, Connection, Window};
use crate::{
    Appearance, Clipboard, CursorIcon, DeadKeyStatus, Dimensions, MouseButtons, MouseEvent,
    MouseEventKind, MousePress, Point, Rect, RequestedWindowGeometry, ResizeIncrement,
    ResolvedGeometry, ScreenPoint, ScreenRect, WindowDecorations, WindowEvent, WindowEventSender,
    WindowOps, WindowState,
};
use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use config::ConfigHandle;
use promise::{Future, Promise};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle, XcbDisplayHandle, XcbWindowHandle,
};
use std::any::Any;
use std::convert::TryInto;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use url::Url;
use wezterm_font::FontConfiguration;
use wezterm_input_types::{KeyCode, KeyEvent, KeyboardLedStatus, Modifiers};
use xcb::x::{Atom, PropMode};
use xcb::{Event, Xid};

#[derive(Default)]
struct CopyAndPaste {
    clipboard_owned: Option<String>,
    primary_selection_owned: Option<String>,
    clipboard_request: Option<Promise<String>>,
    selection_request: Option<Promise<String>>,
    time: u32,
}

impl CopyAndPaste {
    fn clipboard(&self, clipboard: Clipboard) -> &Option<String> {
        match clipboard {
            Clipboard::PrimarySelection => &self.primary_selection_owned,
            Clipboard::Clipboard => &self.clipboard_owned,
        }
    }

    fn clipboard_mut(&mut self, clipboard: Clipboard) -> &mut Option<String> {
        match clipboard {
            Clipboard::PrimarySelection => &mut self.primary_selection_owned,
            Clipboard::Clipboard => &mut self.clipboard_owned,
        }
    }

    fn request_mut(&mut self, clipboard: Clipboard) -> &mut Option<Promise<String>> {
        match clipboard {
            Clipboard::PrimarySelection => &mut self.selection_request,
            Clipboard::Clipboard => &mut self.clipboard_request,
        }
    }
}

struct DragAndDrop {
    src_window: Option<xcb::x::Window>,
    src_types: Vec<Atom>,
    src_action: Atom,
    time: u32,
    target_type: Atom,
    target_action: Atom,
}

impl Default for DragAndDrop {
    fn default() -> DragAndDrop {
        DragAndDrop {
            src_window: None,
            src_types: Vec::new(),
            src_action: xcb::x::ATOM_NONE,
            time: 0,
            target_type: xcb::x::ATOM_NONE,
            target_action: xcb::x::ATOM_NONE,
        }
    }
}

/// What a fit depends on: the outer size, restored, tiled, the dpi's bits.
type FitKey = ((u16, u16), bool, bool, u64);

pub(crate) struct XWindowInner {
    pub window_id: xcb::x::Window,
    pub child_id: xcb::x::Window,
    conn: Weak<XConnection>,
    pub events: WindowEventSender,
    width: u16,
    height: u16,
    last_wm_state: WindowState,
    dpi: f64,
    cursors: CursorInfo,
    copy_and_paste: CopyAndPaste,
    drag_and_drop: DragAndDrop,
    config: ConfigHandle,
    appearance: Appearance,
    title: String,
    pub has_focus: Option<bool>,
    verify_focus: bool,
    last_cursor_position: Rect,
    invalidated: bool,
    paint_throttled: bool,
    pending: Vec<WindowEvent>,
    sure_about_geometry: bool,
    current_mouse_event: Option<MouseEvent>,
    window_drag_position: Option<ScreenPoint>,
    dragging: bool,
    outstanding_configure_requests: usize,
    pending_finished_resizes: usize,
    /// The desktop theme's edge around the content (see `edge`), where
    /// this window draws its own.
    edge: Option<crate::os::edge::Edge>,
    /// The margins it takes now: none maximized or full screen. `width`
    /// and `height` are the content's; the X window is this much larger.
    insets: crate::os::edge::Insets,
    /// The X window's own size.
    outer: (u16, u16),
    /// What the margins were last painted for: size, insets, focus, dpi.
    edge_painted: Option<((u16, u16), crate::os::edge::Insets, bool, u64)>,
    /// The last `fit`: the outer size, restored, tiled and dpi it was for,
    /// and the content's size it gave. Fitting again to the same is a
    /// no-op (a drag's ConfigureNotify, a repeated state).
    fitted: Option<(FitKey, (u16, u16))>,
    /// Maximized one way: the edge is the resize band alone.
    tiled: bool,
    edge_gc: Option<xcb::x::Gcontext>,
    /// The margins as last painted, a pixmap per rectangle (x, y, width,
    /// height) kept on the server: an Expose copies them back rather than
    /// uploading them again
    edge_pixmaps: Vec<(u16, u16, u16, u16, xcb::x::Pixmap)>,
    /// The cursor the GUI last asked for, and whether the pointer is on
    /// the margins, showing a resize cursor of ours instead.
    gui_cursor: Option<CursorIcon>,
    on_edge: bool,
}

/// <https://specifications.freedesktop.org/wm-spec/wm-spec-latest.html#idm46409506331616>
const _NET_WM_MOVERESIZE_MOVE: u32 = 8;
const _NET_WM_MOVERESIZE_CANCEL: u32 = 11;

impl Drop for XWindowInner {
    fn drop(&mut self) {
        if self.window_id != xcb::x::Window::none() {
            if let Some(conn) = self.conn.upgrade() {
                self.conn()
                    .conn()
                    .flush()
                    .context("flush pending requests prior to issuing DestroyWindow")
                    .ok();
                conn.send_request_no_reply_log(&xcb::x::DestroyWindow {
                    window: self.child_id,
                });
                conn.send_request_no_reply_log(&xcb::x::DestroyWindow {
                    window: self.window_id,
                });
                for (_, _, _, _, pixmap) in self.edge_pixmaps.drain(..) {
                    conn.send_request_no_reply_log(&xcb::x::FreePixmap { pixmap });
                }
                if let Some(gc) = self.edge_gc.take() {
                    conn.send_request_no_reply_log(&xcb::x::FreeGc { gc });
                }
            }
        }
    }
}

impl HasDisplayHandle for XWindowInner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        if let Some(conn) = self.conn.upgrade() {
            let handle =
                XcbDisplayHandle::new(NonNull::new(conn.conn.get_raw_conn() as _), conn.screen_num);
            unsafe { Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Xcb(handle))) }
        } else {
            Err(HandleError::Unavailable)
        }
    }
}

impl HasWindowHandle for XWindowInner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let mut handle =
            XcbWindowHandle::new(NonZeroU32::new(self.child_id.resource_id()).expect("non-zero"));
        handle.visual_id = NonZeroU32::new(self.conn.upgrade().unwrap().visual.visual_id());
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Xcb(handle))) }
    }
}

impl XWindowInner {
    fn enable_opengl(&mut self) -> anyhow::Result<Rc<glium::backend::Context>> {
        let conn = self.conn();

        let gl_state = match conn.gl_connection.borrow().as_ref() {
            None => crate::egl::GlState::create(
                Some(conn.conn.get_raw_dpy() as *const _),
                self.child_id.resource_id() as *mut _,
            ),
            Some(glconn) => crate::egl::GlState::create_with_existing_connection(
                glconn,
                self.child_id.resource_id() as *mut _,
            ),
        };

        // Don't chain on the end of the above to avoid borrowing gl_connection twice.
        let gl_state = gl_state.map(Rc::new).and_then(|state| unsafe {
            conn.gl_connection
                .borrow_mut()
                .replace(Rc::clone(state.get_connection()));
            Ok(glium::backend::Context::new(
                Rc::clone(&state),
                true,
                if cfg!(debug_assertions) {
                    glium::debug::DebugCallbackBehavior::DebugMessageOnError
                } else {
                    glium::debug::DebugCallbackBehavior::Ignore
                },
            )?)
        })?;

        Ok(gl_state)
    }

    /// Add a region to the list of exposed/damaged/dirty regions.
    /// Note that a window resize will likely invalidate the entire window.
    /// If the new region intersects with the prior region, then we expand
    /// it to encompass both.  This avoids bloating the list with a series
    /// of increasing rectangles when resizing larger or smaller.
    fn expose(&mut self, x: u16, y: u16, width: u16, height: u16, count: u16) {
        log::trace!("expose: {x},{y} {width}x{height} ({count} expose events follow this one)");
        let max_x = x.saturating_add(width);
        let max_y = y.saturating_add(height);
        if max_x > self.width || max_y > self.height {
            log::trace!(
                "flagging geometry as unsure because exposed region is larger than known geom"
            );
            self.sure_about_geometry = false;
        }
        self.queue_pending(WindowEvent::NeedRepaint);
    }

    fn cancel_drag(&mut self) -> bool {
        if self.dragging {
            log::debug!("cancel_drag");
            self.net_wm_moveresize(0, 0, _NET_WM_MOVERESIZE_CANCEL, 0);
            self.dragging = false;
            if let Some(event) = self.current_mouse_event.take() {
                self.do_mouse_event(MouseEvent {
                    kind: MouseEventKind::Release(MousePress::Left),
                    ..event
                })
                .ok();
            }
            return true;
        }
        false
    }

    fn do_mouse_event(&mut self, event: MouseEvent) -> anyhow::Result<()> {
        if self.cancel_drag() {
            return Ok(());
        }
        self.current_mouse_event.replace(event.clone());
        self.events.dispatch(WindowEvent::MouseEvent(event));
        Ok(())
    }

    fn set_cursor(&mut self, cursor: Option<CursorIcon>) -> anyhow::Result<()> {
        self.gui_cursor = cursor;
        if self.on_edge {
            return Ok(());
        }
        self.cursors.set_cursor(self.window_id, cursor)
    }

    fn check_dpi_and_synthesize_resize(&mut self) {
        let conn = self.conn();
        let dpi = conn.default_dpi();

        if dpi != self.dpi {
            log::trace!(
                "dpi changed from {} -> {}, so synthesize a resize",
                dpi,
                self.dpi
            );
            self.dpi = dpi;
            self.last_wm_state = self.get_window_state().unwrap_or(WindowState::default());
            self.events.dispatch(WindowEvent::Resized {
                dimensions: Dimensions {
                    pixel_width: self.width as usize,
                    pixel_height: self.height as usize,
                    dpi: self.dpi as usize,
                },
                window_state: self.last_wm_state,
                live_resizing: false,
            });
        }
    }

    fn queue_pending(&mut self, event: WindowEvent) {
        self.pending.push(event);
    }

    fn resize_child(&self, width: u32, height: u32) {
        self.conn()
            .send_request_no_reply_log(&xcb::x::ConfigureWindow {
                window: self.child_id,
                value_list: &[
                    xcb::x::ConfigWindow::X(i32::from(self.insets.left)),
                    xcb::x::ConfigWindow::Y(i32::from(self.insets.top)),
                    xcb::x::ConfigWindow::Width(width as u32),
                    xcb::x::ConfigWindow::Height(height as u32),
                ],
            });
        // send_request_no_reply_log() is synchronous, so no further synchronization required
    }

    fn restored(state: WindowState) -> bool {
        !state.intersects(WindowState::MAXIMIZED | WindowState::FULL_SCREEN | WindowState::TILED)
    }

    /// Fits the content into an `outer`-sized X window in `state`: the
    /// edge's margins taken off (none without an edge, or maximized),
    /// the child placed and shaped, the window manager told; the
    /// content's size. The margins are painted when anything they show
    /// changed.
    fn fit(&mut self, outer: (u16, u16), state: WindowState, dpi: f64) -> (u16, u16) {
        self.outer = outer;
        let Some(edge) = self.edge.as_ref() else {
            return outer;
        };
        let scale = dpi / 96.;
        let restored = Self::restored(state);
        // tiled (maximized one way): the resize band, no shadow, no round
        // corners, as Chrome keeps its frame then
        let tiled = state.contains(WindowState::TILED);
        let key = (outer, restored, tiled, dpi.to_bits());
        if let Some((fitted, inner)) = self.fitted {
            if fitted == key {
                return inner;
            }
        }
        self.tiled = tiled;
        let insets = if tiled {
            edge.band_insets(scale)
        } else {
            edge.insets(scale, restored)
        };
        let (radius, band) = (edge.radius(scale), edge.band(scale));
        let inner = (
            outer.0.saturating_sub(insets.left + insets.right).max(1),
            outer.1.saturating_sub(insets.top + insets.bottom).max(1),
        );
        self.insets = insets;
        let conn = self.conn();
        // where the window really is, for the window manager (Chrome
        // sets it on mapping whatever the manager knows, the solid
        // frame's 4 DIP included: X11Window::UpdateDecorationInsets)
        let extents = conn.atom_gtk_frame_extents;
        if insets.is_empty() {
            conn.send_request_unchecked(&xcb::x::DeleteProperty {
                window: self.window_id,
                property: extents,
            });
        } else {
            conn.send_request_unchecked(&xcb::x::ChangeProperty {
                mode: PropMode::Replace,
                window: self.window_id,
                property: extents,
                r#type: xcb::x::ATOM_CARDINAL,
                data: &[
                    u32::from(insets.left),
                    u32::from(insets.right),
                    u32::from(insets.top),
                    u32::from(insets.bottom),
                ],
            });
        }
        // where the window is opaque, for the compositor: the content but
        // for its round corners (Chrome's UpdateFrameRegions, the clip
        // region less its corners); with the solid frame the clip is the
        // whole window, its borders opaque too (Chrome's window on
        // deepin: 8,0,833,8 0,8,849,500 for 849 x 508)
        let corner = if restored { u32::from(radius) } else { 0 }
            .min(u32::from(inner.0) / 2)
            .min(u32::from(inner.1));
        let (l, t, w, h) = if edge.is_solid() {
            (0, 0, u32::from(outer.0), u32::from(outer.1))
        } else {
            (
                u32::from(insets.left),
                u32::from(insets.top),
                u32::from(inner.0),
                u32::from(inner.1),
            )
        };
        let opaque: Vec<u32> = if corner == 0 {
            vec![l, t, w, h]
        } else {
            vec![
                l,
                t + corner,
                w,
                h - corner,
                l + corner,
                t,
                w - 2 * corner,
                corner,
            ]
        };
        conn.send_request_unchecked(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: self.window_id,
            property: conn.atom_net_wm_opaque_region,
            r#type: xcb::x::ATOM_CARDINAL,
            data: &opaque,
        });
        // clicks on the shadow fall through, but for the resize band
        if insets.is_empty() {
            conn.send_request_unchecked(&xcb::shape::Mask {
                operation: xcb::shape::So::Set,
                destination_kind: xcb::shape::Sk::Input,
                destination_window: self.window_id,
                x_offset: 0,
                y_offset: 0,
                source_bitmap: xcb::x::Pixmap::none(),
            });
        } else {
            let x = i32::from(insets.left) - i32::from(band);
            let y = i32::from(insets.top) - i32::from(band);
            conn.send_request_unchecked(&xcb::shape::Rectangles {
                operation: xcb::shape::So::Set,
                destination_kind: xcb::shape::Sk::Input,
                ordering: xcb::x::ClipOrdering::Unsorted,
                destination_window: self.window_id,
                x_offset: 0,
                y_offset: 0,
                rectangles: &[xcb::x::Rectangle {
                    x: x.max(0) as i16,
                    y: y.max(0) as i16,
                    width: (i32::from(inner.0) + 2 * i32::from(band)).min(i32::from(outer.0))
                        as u16,
                    height: (i32::from(inner.1) + 2 * i32::from(band)).min(i32::from(outer.1))
                        as u16,
                }],
            });
        }
        // the content's top corners cut, for the round ones painted
        // beneath (restored only)
        let r = if restored {
            radius.min(inner.0 / 2).min(inner.1)
        } else {
            0
        };
        if r == 0 {
            conn.send_request_unchecked(&xcb::shape::Mask {
                operation: xcb::shape::So::Set,
                destination_kind: xcb::shape::Sk::Bounding,
                destination_window: self.child_id,
                x_offset: 0,
                y_offset: 0,
                source_bitmap: xcb::x::Pixmap::none(),
            });
        } else {
            // the arc cut, row by row: only what lies outside it. A
            // square of r cut away whatever the content draws by the
            // corner inside the arc too (a caption button's hover disc
            // lost its corner); Chrome clips to the rounded shape
            let mut rectangles: Vec<xcb::x::Rectangle> = crate::os::edge::corner_row_insets(r)
                .into_iter()
                .enumerate()
                .map(|(y, inset)| xcb::x::Rectangle {
                    x: inset as i16,
                    y: y as i16,
                    width: inner.0.saturating_sub(2 * inset),
                    height: 1,
                })
                .collect();
            rectangles.push(xcb::x::Rectangle {
                x: 0,
                y: r as i16,
                width: inner.0,
                height: inner.1 - r,
            });
            conn.send_request_unchecked(&xcb::shape::Rectangles {
                operation: xcb::shape::So::Set,
                destination_kind: xcb::shape::Sk::Bounding,
                ordering: xcb::x::ClipOrdering::YSorted,
                destination_window: self.child_id,
                x_offset: 0,
                y_offset: 0,
                rectangles: &rectangles,
            });
        }
        let _ = conn.flush();
        self.resize_child(u32::from(inner.0), u32::from(inner.1));
        self.paint_edge(dpi);
        self.fitted = Some((key, inner));
        inner
    }

    /// Paints the margins (and the round top corners) with the theme's
    /// edge, focused or not, unless they show that already.
    fn paint_edge(&mut self, dpi: f64) {
        let focused = self.has_focus.unwrap_or(true);
        let key = (self.outer, self.insets, focused, dpi.to_bits());
        if self.edge.is_none() || self.edge_painted == Some(key) {
            return;
        }
        self.edge_painted = Some(key);
        let header: config::SrgbaTuple = if focused {
            self.config.window_frame.active_titlebar_bg.into()
        } else {
            self.config.window_frame.inactive_titlebar_bg.into()
        };
        let header = [header.0, header.1, header.2, header.3];
        let (outer, insets) = (self.outer, self.insets);
        let tiled = self.tiled;
        let rects = match self.edge.as_mut() {
            Some(_) if tiled => crate::os::edge::Edge::clear(outer, insets),
            Some(edge) => edge.paint(focused, outer, insets, dpi / 96., header),
            None => return,
        };
        let conn = self.conn();
        let gc = match self.edge_gc {
            Some(gc) => gc,
            None => {
                let gc = conn.generate_id();
                conn.send_request_no_reply_log(&xcb::x::CreateGc {
                    cid: gc,
                    drawable: xcb::x::Drawable::Window(self.window_id),
                    value_list: &[],
                });
                self.edge_gc = Some(gc);
                gc
            }
        };
        for (_, _, _, _, pixmap) in self.edge_pixmaps.drain(..) {
            conn.send_request_unchecked(&xcb::x::FreePixmap { pixmap });
        }
        for (x, y, w, h, data) in rects {
            if w == 0 || h == 0 {
                continue;
            }
            // each rectangle goes to a pixmap of its own, then to the
            // window: an Expose copies it again (`expose_edge`)
            let pixmap = conn.generate_id();
            conn.send_request_unchecked(&xcb::x::CreatePixmap {
                depth: 32,
                pid: pixmap,
                drawable: xcb::x::Drawable::Window(self.window_id),
                width: w,
                height: h,
            });
            // a few rows per request, well inside any request size limit
            let row = usize::from(w) * 4;
            let rows = (256 * 1024 / row.max(1)).max(1);
            for (i, chunk) in data.chunks(row * rows).enumerate() {
                conn.send_request_unchecked(&xcb::x::PutImage {
                    format: xcb::x::ImageFormat::ZPixmap,
                    drawable: xcb::x::Drawable::Pixmap(pixmap),
                    gc,
                    width: w,
                    height: (chunk.len() / row) as u16,
                    dst_x: 0,
                    dst_y: (i * rows) as i16,
                    left_pad: 0,
                    depth: 32,
                    data: chunk,
                });
            }
            conn.send_request_unchecked(&xcb::x::CopyArea {
                src_drawable: xcb::x::Drawable::Pixmap(pixmap),
                dst_drawable: xcb::x::Drawable::Window(self.window_id),
                gc,
                src_x: 0,
                src_y: 0,
                dst_x: x as i16,
                dst_y: y as i16,
                width: w,
                height: h,
            });
            self.edge_pixmaps.push((x, y, w, h, pixmap));
        }
        let _ = conn.flush();
    }

    /// Repaints the exposed part of the margins from the server's copy
    /// of them (`edge_pixmaps`); `false` when there is none to copy from
    /// yet. Uploading them again, as every Expose did, sent the whole
    /// edge's pixels (the shadow's too) each time another window moved
    /// off this one.
    fn expose_edge(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        let (Some(gc), false) = (self.edge_gc, self.edge_pixmaps.is_empty()) else {
            return false;
        };
        if self.edge_painted.map(|(outer, ..)| outer) != Some(self.outer) {
            return false;
        }
        let (ex0, ey0) = (u32::from(x), u32::from(y));
        let (ex1, ey1) = (ex0 + u32::from(width), ey0 + u32::from(height));
        let conn = self.conn();
        for &(rx, ry, rw, rh, pixmap) in &self.edge_pixmaps {
            let (rx0, ry0) = (u32::from(rx), u32::from(ry));
            let (rx1, ry1) = (rx0 + u32::from(rw), ry0 + u32::from(rh));
            let (x0, y0, x1, y1) = (ex0.max(rx0), ey0.max(ry0), ex1.min(rx1), ey1.min(ry1));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            conn.send_request_unchecked(&xcb::x::CopyArea {
                src_drawable: xcb::x::Drawable::Pixmap(pixmap),
                dst_drawable: xcb::x::Drawable::Window(self.window_id),
                gc,
                src_x: (x0 - rx0) as i16,
                src_y: (y0 - ry0) as i16,
                dst_x: x0 as i16,
                dst_y: y0 as i16,
                width: (x1 - x0) as u16,
                height: (y1 - y0) as u16,
            });
        }
        let _ = conn.flush();
        log::trace!("edge: exposed {x},{y} {width}x{height} copied from the server's copy");
        true
    }

    /// Pointer coordinates in the X window to the content's; on the
    /// margins `None`, after showing the resize cursor for the band there
    /// (or the plain one past it, on the shadow).
    fn to_content(&mut self, x: i16, y: i16) -> Option<(isize, isize)> {
        if self.insets.is_empty() {
            return Some((x.into(), y.into()));
        }
        let (cx, cy) = (
            isize::from(x) - self.insets.left as isize,
            isize::from(y) - self.insets.top as isize,
        );
        let inside = cx >= 0 && cy >= 0 && cx < self.width as isize && cy < self.height as isize;
        // Chrome's solid frame has no band above: the content's top 4 DIP
        // resize instead (kResizeTopBorderThickness)
        let top_band = self
            .edge
            .as_ref()
            .filter(|e| e.is_solid() && !self.tiled)
            .map(|e| e.band(self.dpi / 96.) as isize)
            .unwrap_or(0);
        if inside && cy >= top_band {
            if self.on_edge {
                self.on_edge = false;
                let _ = self.cursors.set_cursor(self.window_id, self.gui_cursor);
            }
            return Some((cx, cy));
        }
        let band = self
            .edge
            .as_ref()
            .map(|e| e.band(self.dpi / 96.))
            .unwrap_or_default();
        let side = if inside {
            Some(crate::ResizeEdge::Top)
        } else {
            crate::os::edge::side_at(x.into(), y.into(), self.outer, self.insets, band)
        };
        let cursor = side.map(|side| {
            use crate::ResizeEdge as E;
            match side {
                E::TopLeft => CursorIcon::NwResize,
                E::Top => CursorIcon::NResize,
                E::TopRight => CursorIcon::NeResize,
                E::Right => CursorIcon::EResize,
                E::BottomRight => CursorIcon::SeResize,
                E::Bottom => CursorIcon::SResize,
                E::BottomLeft => CursorIcon::SwResize,
                E::Left => CursorIcon::WResize,
            }
        });
        self.on_edge = true;
        let _ = self
            .cursors
            .set_cursor(self.window_id, cursor.or(Some(CursorIcon::Default)));
        None
    }

    pub fn dispatch_pending_events(&mut self) -> anyhow::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let mut need_paint = false;
        let mut resize = None;

        for event in self.pending.drain(..) {
            match event {
                WindowEvent::NeedRepaint => {
                    if need_paint {
                        log::trace!("coalesce a repaint");
                    }
                    need_paint = true;
                }
                e @ WindowEvent::Resized { .. } => {
                    if resize.is_some() {
                        log::trace!("coalesce a resize");
                    }
                    resize.replace(e);
                }
                e => {
                    self.events.dispatch(e);
                }
            }
        }

        if let Some(resize) = resize.take() {
            self.sure_about_geometry = true;
            self.events.dispatch(resize);
        }

        // These SetInnerSizeCompleted events need to be dispatched after the
        // above Resized events because a resize cannot finish before it occurs.
        while self.pending_finished_resizes > 0 {
            self.events.dispatch(WindowEvent::SetInnerSizeCompleted);
            self.pending_finished_resizes -= 1;
        }

        if need_paint {
            if self.paint_throttled {
                self.invalidated = true;
            } else {
                self.invalidated = false;

                if self.verify_focus || self.has_focus.is_none() {
                    log::trace!("About to paint, but we're unsure about focus; querying!");

                    let focus = self
                        .conn()
                        .send_and_wait_request(&xcb::x::GetInputFocus {})?;
                    let focused = focus.focus() == self.window_id;
                    log::trace!(
                        "Do I {:?} have focus? result={}, I thought {:?}",
                        self.window_id,
                        focused,
                        self.has_focus
                    );
                    if Some(focused) != self.has_focus {
                        self.has_focus.replace(focused);
                        self.events.dispatch(WindowEvent::FocusChanged(focused));
                    }

                    self.verify_focus = false;
                }

                if !self.sure_about_geometry {
                    self.sure_about_geometry = true;

                    log::trace!(
                        "About to paint, but we're unsure about geometry; querying window_id {:?}!",
                        self.window_id
                    );
                    let geom = self
                        .conn()
                        .send_and_wait_request(&xcb::x::GetGeometry {
                            drawable: xcb::x::Drawable::Window(self.window_id),
                        })
                        .context("querying geometry")?;
                    log::trace!(
                        "geometry is {}x{} vs. our initial {}x{}",
                        geom.width(),
                        geom.height(),
                        self.width,
                        self.height
                    );

                    let window_state = self.get_window_state().unwrap_or(WindowState::default());
                    let (width, height) =
                        self.fit((geom.width(), geom.height()), window_state, self.dpi);

                    if self.width != width
                        || self.height != height
                        || self.last_wm_state != window_state
                    {
                        self.resize_child(width as u32, height as u32);

                        self.width = width;
                        self.height = height;
                        self.last_wm_state = window_state;

                        self.events.dispatch(WindowEvent::Resized {
                            dimensions: Dimensions {
                                pixel_width: self.width as usize,
                                pixel_height: self.height as usize,
                                dpi: self.dpi as usize,
                            },
                            window_state,
                            live_resizing: false,
                        });
                    }
                }

                // the next frame is due an interval after this one began,
                // not after it ended (a frame then takes the interval, not
                // the interval and the paint)
                let began = std::time::Instant::now();
                self.events.dispatch(WindowEvent::NeedRepaint);

                self.paint_throttled = true;
                let window_id = self.window_id;
                let max_fps = self.config.max_fps.max(1);
                promise::spawn::spawn(async move {
                    async_io::Timer::at(
                        began + std::time::Duration::from_micros(1_000_000 / max_fps),
                    )
                    .await;
                    XConnection::with_window_inner(window_id, move |inner| {
                        inner.paint_throttled = false;
                        if inner.invalidated {
                            inner.invalidate();
                        }
                        Ok(())
                    });
                })
                .detach();
            }
        }

        Ok(())
    }

    fn button_event(
        &mut self,
        pressed: bool,
        time: xcb::x::Timestamp,
        detail: xcb::x::Button,
        event_x: i16,
        event_y: i16,
        root_x: i16,
        root_y: i16,
        state: xcb::x::KeyButMask,
    ) -> anyhow::Result<()> {
        self.copy_and_paste.time = time;

        if self.cancel_drag() {
            log::debug!("cancel drag due to button {detail} {state:?}");
            return Ok(());
        }

        let kind = match detail {
            b @ 1..=3 => {
                let button = match b {
                    1 => MousePress::Left,
                    2 => MousePress::Middle,
                    3 => MousePress::Right,
                    _ => unreachable!(),
                };
                if pressed {
                    MouseEventKind::Press(button)
                } else {
                    MouseEventKind::Release(button)
                }
            }
            b @ 4..=5 => {
                if !pressed {
                    return Ok(());
                }

                // Ideally this would be configurable, but it's currently a bit
                // awkward to configure this layer, so let's just improve the
                // default for now!
                const LINES_PER_TICK: i16 = 5;

                MouseEventKind::VertWheel(if b == 4 {
                    LINES_PER_TICK
                } else {
                    -LINES_PER_TICK
                })
            }
            _ => {
                log::trace!("button {} is not implemented", detail);
                return Ok(());
            }
        };

        let Some((x, y)) = self.to_content(event_x, event_y) else {
            // the margins: the band resizes, the window manager does it
            if matches!(kind, MouseEventKind::Press(MousePress::Left)) {
                let band = self
                    .edge
                    .as_ref()
                    .map(|e| e.band(self.dpi / 96.))
                    .unwrap_or_default();
                if let Some(side) = crate::os::edge::side_at(
                    event_x.into(),
                    event_y.into(),
                    self.outer,
                    self.insets,
                    band,
                ) {
                    self.window_drag_position =
                        Some(ScreenPoint::new(root_x.into(), root_y.into()));
                    self.request_drag_resize(side)?;
                }
            }
            return Ok(());
        };
        let event = MouseEvent {
            kind,
            coords: Point::new(x, y),
            screen_coords: ScreenPoint::new(root_x.try_into().unwrap(), root_y.try_into().unwrap()),
            modifiers: xkeysyms::modifiers_from_state(state.bits()),
            mouse_buttons: MouseButtons::default(),
        };
        self.do_mouse_event(event)
    }

    fn configure_notify(&mut self, source: &str, width: u16, height: u16) -> anyhow::Result<()> {
        let conn = self.conn();

        self.update_ime_position();

        let mut dpi = conn.default_dpi();

        if !self.config.dpi_by_screen.is_empty() {
            let coords = conn
                .send_and_wait_request(&xcb::x::TranslateCoordinates {
                    src_window: self.window_id,
                    dst_window: conn.root,
                    src_x: 0,
                    src_y: 0,
                })
                .context("querying window coordinates")?;
            let screens = conn.get_cached_screens()?;
            let window_rect: ScreenRect = euclid::rect(
                coords.dst_x().into(),
                coords.dst_y().into(),
                width as isize,
                height as isize,
            );
            let screen = screens
                .by_name
                .values()
                .filter_map(|screen| {
                    screen
                        .rect
                        .intersection(&window_rect)
                        .map(|r| (screen, r.area()))
                })
                .max_by_key(|s| s.1)
                .ok_or_else(|| anyhow::anyhow!("window is not in any screen"))?
                .0;

            if let Some(value) = self.config.dpi_by_screen.get(&screen.name).copied() {
                dpi = value;
            } else if let Some(value) = self.config.dpi {
                dpi = value;
            }
        }

        // a drag moves the window: the same size, the same dpi, nothing to
        // fit, paint or report (and no round trip to ask for the state:
        // a state change comes as a PropertyNotify, which has the next
        // paint query it)
        if self.edge.is_some() && (width, height) == self.outer && dpi == self.dpi {
            return Ok(());
        }

        // the X window's size in, the content's from here on
        let state = self.get_window_state().unwrap_or(WindowState::default());
        let (width, height) = self.fit((width, height), state, dpi);

        if width == self.width && height == self.height && dpi == self.dpi {
            // Effectively unchanged; perhaps it was simply moved?
            // Do nothing!
            log::trace!(
                "Ignoring {source} ({width}x{height} dpi={dpi}) \
                                 because width,height,dpi are unchanged",
            );
            return Ok(());
        }

        if self.edge.is_none() {
            // (with an edge, fit placed the child)
            self.resize_child(width as u32, height as u32);
        }

        log::trace!(
            "{source}: width {} -> {}, height {} -> {}, dpi {} -> {}",
            self.width,
            width,
            self.height,
            height,
            self.dpi,
            dpi
        );

        self.width = width;
        self.height = height;
        self.dpi = dpi;
        self.last_wm_state = state;

        let dimensions = Dimensions {
            pixel_width: self.width as usize,
            pixel_height: self.height as usize,
            dpi: self.dpi as usize,
        };

        self.queue_pending(WindowEvent::Resized {
            dimensions,
            window_state: self.last_wm_state,
            // Assume that we're live resizing: we don't know for sure,
            // but it seems like a reasonable assumption
            live_resizing: true,
        });
        Ok(())
    }

    fn xdnd_event(&mut self, msgtype: Atom, data: &[u32]) -> anyhow::Result<()> {
        use xcb::XidNew;
        let conn = self.conn();
        let msgtype_name = conn.atom_name(msgtype);
        let srcwin = xcb::x::Window::new(data[0]);
        if msgtype == conn.atom_xdndenter {
            self.drag_and_drop.src_window = Some(srcwin);
            let moretypes = data[1] & 0x01 != 0;
            let xdndversion = data[1] >> 24 as u8;
            log::trace!("ClientMessage {msgtype_name}, Version {xdndversion}, more than 3 types: {moretypes}");
            if !moretypes {
                self.drag_and_drop.src_types = data[2..]
                    .into_iter()
                    .filter(|&&x| x != 0)
                    .map(|&x| Atom::new(x))
                    .collect();
            } else {
                self.drag_and_drop.src_types =
                    match conn.send_and_wait_request(&xcb::x::GetProperty {
                        delete: false,
                        window: srcwin,
                        property: conn.atom_xdndtypelist,
                        r#type: xcb::x::ATOM_ATOM,
                        long_offset: 0,
                        long_length: u32::max_value(),
                    }) {
                        Ok(prop) => prop.value::<Atom>().to_vec(),
                        Err(err) => {
                            log::error!(
                                "xdnd: unable to get type list from source window: {:?}",
                                err
                            );
                            Vec::<Atom>::new()
                        }
                    };
            }
            self.drag_and_drop.target_type = xcb::x::ATOM_NONE;
            for t in [
                conn.atom_texturilist,
                conn.atom_xmozurl,
                conn.atom_utf8_string,
            ] {
                if self.drag_and_drop.src_types.contains(&t) {
                    self.drag_and_drop.target_type = t;
                    break;
                }
            }
            for t in &self.drag_and_drop.src_types {
                log::trace!("types offered: {}", conn.atom_name(*t));
            }
            log::trace!(
                "selected: {}",
                conn.atom_name(self.drag_and_drop.target_type)
            );
        } else if self.drag_and_drop.src_window != Some(srcwin) {
            log::error!("ClientMessage {msgtype_name} received, but no Xdnd in progress or source window mismatch");
        } else if msgtype == conn.atom_xdndposition {
            self.drag_and_drop.time = data[3];
            let (x, y) = (data[2] >> 16 as u16, data[2] as u16);
            self.drag_and_drop.src_action = Atom::new(data[4]);
            self.drag_and_drop.target_action = conn.atom_xdndactioncopy;
            log::trace!(
                "ClientMessage {msgtype_name}, ({x}, {y}), timestamp: {}, action: {}",
                self.drag_and_drop.time,
                conn.atom_name(self.drag_and_drop.src_action)
            );
            conn.send_request_no_reply_log(&xcb::x::SendEvent {
                propagate: false,
                destination: xcb::x::SendEventDest::Window(srcwin),
                event_mask: xcb::x::EventMask::empty(),
                event: &xcb::x::ClientMessageEvent::new(
                    srcwin,
                    conn.atom_xdndstatus,
                    xcb::x::ClientMessageData::Data32([
                        self.window_id.resource_id(),
                        2 | (self.drag_and_drop.target_type != xcb::x::ATOM_NONE) as u32,
                        0,
                        0,
                        self.drag_and_drop.target_action.resource_id(),
                    ]),
                ),
            });
        } else if msgtype == conn.atom_xdndleave {
            self.drag_and_drop.src_window = None;
            log::trace!("ClientMessage {msgtype_name}");
        } else if msgtype == conn.atom_xdnddrop {
            self.drag_and_drop.time = data[2];
            log::trace!(
                "ClientMessage {msgtype_name}, timestamp: {}",
                self.drag_and_drop.time
            );
            if self.drag_and_drop.target_type != xcb::x::ATOM_NONE {
                conn.send_request_no_reply_log(&xcb::x::ConvertSelection {
                    requestor: self.window_id,
                    selection: conn.atom_xdndselection,
                    target: self.drag_and_drop.target_type,
                    property: conn.atom_xsel_data,
                    time: self.drag_and_drop.time,
                });
            } else {
                log::warn!("XdndDrop received, but no target type selected. Ignoring.");
                conn.send_request_no_reply_log(&xcb::x::SendEvent {
                    propagate: false,
                    destination: xcb::x::SendEventDest::Window(srcwin),
                    event_mask: xcb::x::EventMask::empty(),
                    event: &xcb::x::ClientMessageEvent::new(
                        srcwin,
                        conn.atom_xdndfinished,
                        xcb::x::ClientMessageData::Data32([
                            self.window_id.resource_id(),
                            0,
                            0,
                            0,
                            0,
                        ]),
                    ),
                });
            }
        }
        return Ok(());
    }

    pub fn dispatch_event(&mut self, event: &Event) -> anyhow::Result<()> {
        let conn = self.conn();
        match event {
            Event::X(xcb::x::Event::Expose(expose)) if expose.window() == self.window_id => {
                // the margins, which are ours to paint: copied back from
                // the server's copy, or painted afresh without one
                if !self.expose_edge(expose.x(), expose.y(), expose.width(), expose.height())
                    && expose.count() == 0
                {
                    self.edge_painted = None;
                    self.paint_edge(self.dpi);
                }
            }
            Event::X(xcb::x::Event::Expose(expose)) => {
                self.expose(
                    expose.x(),
                    expose.y(),
                    expose.width(),
                    expose.height(),
                    expose.count(),
                );
            }
            Event::Present(xcb::present::Event::ConfigureNotify(cfg)) => {
                self.configure_notify("Present::ConfigureNotify", cfg.width(), cfg.height())?;
            }
            Event::X(xcb::x::Event::ConfigureNotify(cfg)) => {
                self.configure_notify("X::ConfigureNotify", cfg.width(), cfg.height())?;
                if self.outstanding_configure_requests > 0 {
                    self.outstanding_configure_requests -= 1;
                    self.pending_finished_resizes += 1;
                }
            }
            Event::X(xcb::x::Event::KeyPress(key_press)) => {
                self.copy_and_paste.time = key_press.time();
                conn.keyboard
                    .process_key_press_event(key_press, &mut self.events);
            }
            Event::X(xcb::x::Event::KeyRelease(key_release)) => {
                self.copy_and_paste.time = key_release.time();
                conn.keyboard
                    .process_key_release_event(key_release, &mut self.events);
            }
            Event::X(xcb::x::Event::MotionNotify(motion)) => {
                // A move handed to the window manager starts on a motion
                // past the drag threshold, and motion queued before the
                // manager's grab still arrives: with the button still
                // held it is no sign that the move is over (any mouse
                // event cancels it, see do_mouse_event), only stale
                if self.dragging && motion.state().contains(xcb::x::KeyButMask::BUTTON1) {
                    return Ok(());
                }
                let Some((x, y)) = self.to_content(motion.event_x(), motion.event_y()) else {
                    return Ok(());
                };
                let event = MouseEvent {
                    kind: MouseEventKind::Move,
                    coords: Point::new(x, y),
                    screen_coords: ScreenPoint::new(
                        motion.root_x().try_into().unwrap(),
                        motion.root_y().try_into().unwrap(),
                    ),
                    modifiers: xkeysyms::modifiers_from_state(motion.state().bits()),
                    mouse_buttons: MouseButtons::default(),
                };
                self.do_mouse_event(event)?;
            }
            Event::X(xcb::x::Event::ButtonPress(e)) => {
                self.button_event(
                    true,
                    e.time(),
                    e.detail(),
                    e.event_x(),
                    e.event_y(),
                    e.root_x(),
                    e.root_y(),
                    e.state(),
                )?;
            }
            Event::X(xcb::x::Event::ButtonRelease(e)) => {
                self.button_event(
                    false,
                    e.time(),
                    e.detail(),
                    e.event_x(),
                    e.event_y(),
                    e.root_x(),
                    e.root_y(),
                    e.state(),
                )?;
            }
            Event::X(xcb::x::Event::ClientMessage(msg)) => {
                let type_atom_name = conn.atom_name(msg.r#type());
                use xcb::x::ClientMessageData;
                use xcb::XidNew;
                let xdnd_msgtype_atoms = [
                    conn.atom_xdndenter,
                    conn.atom_xdndposition,
                    conn.atom_xdndstatus,
                    conn.atom_xdndleave,
                    conn.atom_xdnddrop,
                    conn.atom_xdndfinished,
                ];
                if xdnd_msgtype_atoms.contains(&msg.r#type()) {
                    if let ClientMessageData::Data32(data) = msg.data() {
                        self.xdnd_event(msg.r#type(), &data)?;
                    } else {
                        log::warn!("Received ClientMessage {type_atom_name} with wrong format");
                    }
                } else if msg.r#type() == conn.atom_protocols {
                    if let ClientMessageData::Data32(data) = msg.data() {
                        let protocol_atom = Atom::new(data[0]);
                        log::trace!(
                            "ClientMessage {type_atom_name}/{}",
                            conn.atom_name(protocol_atom)
                        );
                        if protocol_atom == conn.atom_delete {
                            self.events.dispatch(WindowEvent::CloseRequested);
                        }
                    } else {
                        log::warn!("Received ClientMessage {type_atom_name} with wrong format");
                    }
                }
            }
            Event::X(xcb::x::Event::DestroyNotify(_)) => {
                self.events.dispatch(WindowEvent::Destroyed);
                conn.windows.borrow_mut().remove(&self.window_id);
                conn.child_to_parent_id.borrow_mut().remove(&self.child_id);
            }
            Event::X(xcb::x::Event::SelectionClear(e)) => {
                if let Err(err) = self.selection_clear(e) {
                    log::error!("Error handling SelectionClear: {err:#}");
                }
            }
            Event::X(xcb::x::Event::SelectionRequest(e)) => {
                if let Err(err) = self.selection_request(e) {
                    // Don't propagate this, as it is not worth exiting the program over it.
                    // <https://github.com/wezterm/wezterm/pull/6135>
                    log::error!("Error handling SelectionRequest: {err:#}");
                }
            }
            Event::X(xcb::x::Event::SelectionNotify(e)) => {
                if let Err(err) = self.selection_notify(e) {
                    log::error!("Error handling SelectionNotify: {err:#}");
                }
            }
            Event::X(xcb::x::Event::PropertyNotify(msg)) => {
                let atom_name = conn.atom_name(msg.atom());
                log::trace!("PropertyNotifyEvent {atom_name}");

                if msg.atom() == conn.atom_gtk_edge_constraints {
                    // "_GTK_EDGE_CONSTRAINTS" property is changed when the
                    // accessibility settings change the text size and thus
                    // the dpi.  We use this as a way to detect dpi changes
                    // when running under gnome.
                    conn.update_xrm();
                    self.check_dpi_and_synthesize_resize();
                    let appearance = conn.get_appearance();
                    self.appearance_changed(appearance);
                }

                if msg.atom() == conn.atom_net_wm_state {
                    // Change in window state should be accompanied by
                    // a Configure Notify but not all WMs send these
                    // events consistently/at all/in the same order.
                    self.sure_about_geometry = false;
                    self.verify_focus = true;
                }
            }
            Event::X(xcb::x::Event::FocusIn(e)) => {
                if !matches!(e.detail(), xcb::x::NotifyDetail::Pointer) {
                    self.focus_changed(true);
                }
            }
            Event::X(xcb::x::Event::FocusOut(e)) => {
                if !matches!(e.detail(), xcb::x::NotifyDetail::Pointer) {
                    self.focus_changed(false);
                }
            }
            Event::X(xcb::x::Event::LeaveNotify(_)) => {
                self.events.dispatch(WindowEvent::MouseLeave);
            }
            _ => {
                log::warn!("unhandled: {:?}", event);
            }
        }

        Ok(())
    }

    pub(crate) fn appearance_changed(&mut self, appearance: Appearance) {
        if appearance != self.appearance {
            self.appearance = appearance;
            self.events
                .dispatch(WindowEvent::AppearanceChanged(appearance));
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        log::trace!("focus_changed {focused}, flagging geometry as unsure");
        self.sure_about_geometry = false;
        if self.has_focus != Some(focused) {
            self.has_focus.replace(focused);
            self.update_ime_position();
            log::trace!("Calling focus_change({focused})");
            self.events.dispatch(WindowEvent::FocusChanged(focused));
            self.paint_edge(self.dpi);
        }
    }

    pub fn dispatch_ime_compose_status(&mut self, status: DeadKeyStatus) {
        self.events
            .dispatch(WindowEvent::AdviseDeadKeyStatus(status));
    }

    pub fn dispatch_ime_text(&mut self, text: &str) {
        let key_event = KeyEvent {
            key: KeyCode::Composed(text.into()),
            leds: KeyboardLedStatus::empty(),
            modifiers: Modifiers::NONE,
            repeat_count: 1,
            key_is_down: true,
            raw: None,
        }
        .normalize_shift()
        .resurface_positional_modifier_key();
        self.events.dispatch(WindowEvent::KeyEvent(key_event));
        // Since we just composed, synthesize a cleared status, as we
        // are not guaranteed to receive an event notification to
        // trigger dispatch_ime_compose_status() above.
        // <https://github.com/wezterm/wezterm/issues/4841>
        self.events
            .dispatch(WindowEvent::AdviseDeadKeyStatus(DeadKeyStatus::None));
    }

    /// If we own the selection, make sure that the X server reflects
    /// that and vice versa.
    fn update_selection_owner(&mut self, clipboard: Clipboard) -> anyhow::Result<()> {
        let window_id = self.window_id;
        let conn = self.conn();
        let selection = match clipboard {
            Clipboard::PrimarySelection => xcb::x::ATOM_PRIMARY,
            Clipboard::Clipboard => conn.atom_clipboard,
        };
        let current_owner = conn
            .send_and_wait_request(&xcb::x::GetSelectionOwner { selection })
            .unwrap()
            .owner();

        let we_own_it = self.copy_and_paste.clipboard(clipboard).is_some();

        if !we_own_it && current_owner == window_id {
            log::trace!(
                "SEL: window_id={window_id:?} X thinks we own selection, \
                        but we don't: tell it to clear it"
            );
            // We don't have a selection but X thinks we do; disown it!
            conn.send_request_no_reply(&xcb::x::SetSelectionOwner {
                owner: xcb::x::Window::none(),
                selection,
                // We use CURRENT_TIME rather than self.copy_and_paste.time here, because that field
                // only advances on key/button events on *this* window, so for a window that hasn't
                // been interacted with recently it lags behind the selection's lastTimeChanged.
                // The X server ignores SetSelectionOwner with time before lastTimeChanged, so
                // clipboard updates without explicit interaction (OSC 52) would never be accepted.
                time: xcb::x::CURRENT_TIME,
            })?;
        } else if we_own_it {
            log::trace!(
                "SEL: window_id={window_id:?} currently owned by \
                 {current_owner:?}, tell X we now own it"
            );
            // We have the selection but X doesn't think we do; assert it!
            conn.send_request_no_reply(&xcb::x::SetSelectionOwner {
                owner: self.window_id,
                selection,
                // See note about CURRENT_TIME in the other branch above.
                time: xcb::x::CURRENT_TIME,
            })?;
        } else {
            log::trace!(
                "SEL: window_id={window_id:?} current_owner={current_owner:?} \
                owned={we_own_it}"
            );
        }
        conn.flush().context("flushing after updating selection")?;
        Ok(())
    }

    fn selection_atom_to_clipboard(&self, atom: Atom) -> Option<Clipboard> {
        if atom == xcb::x::ATOM_PRIMARY {
            Some(Clipboard::PrimarySelection)
        } else if atom == self.conn().atom_clipboard {
            Some(Clipboard::Clipboard)
        } else {
            None
        }
    }

    fn selection_clear(&mut self, request: &xcb::x::SelectionClearEvent) -> anyhow::Result<()> {
        let window_id = self.window_id;
        log::debug!("SEL: window_id={window_id:?} {:?}", request);
        if let Some(clipboard) = self.selection_atom_to_clipboard(request.selection()) {
            self.copy_and_paste.clipboard_mut(clipboard).take();
            self.copy_and_paste.request_mut(clipboard).take();
            self.update_selection_owner(clipboard)?;
        }

        Ok(())
    }

    /// A selection request is made to us after we've announced that we own the selection
    /// and when another client wants to copy it.
    fn selection_request(&mut self, request: &xcb::x::SelectionRequestEvent) -> anyhow::Result<()> {
        let conn = self.conn();
        let window_id = self.window_id;
        log::trace!("SEL: window_id={window_id:?} {:?}", request);
        log::trace!(
            "XSEL={:?}, UTF8={:?} PRIMARY={:?} clip={:?}",
            conn.atom_xsel_data,
            conn.atom_utf8_string,
            xcb::x::ATOM_PRIMARY,
            conn.atom_clipboard,
        );

        let selprop = if request.target() == conn.atom_targets {
            // They want to know which targets we support
            let atoms: [Atom; 1] = [conn.atom_utf8_string];
            log::trace!("SEL: window_id={window_id:?} requestor wants supported targets");
            conn.send_request_no_reply(&xcb::x::ChangeProperty {
                mode: PropMode::Replace,
                window: request.requestor(),
                property: request.property(),
                r#type: xcb::x::ATOM_ATOM,
                data: &atoms,
            })?;

            // let the requestor know that we set their property
            request.property()
        } else if request.target() == conn.atom_utf8_string
            || request.target() == xcb::x::ATOM_STRING
        {
            log::trace!("SEL: window_id={window_id:?} requestor wants string data");
            if let Some(clipboard) = self.selection_atom_to_clipboard(request.selection()) {
                // We'll accept requests for UTF-8 or STRING data.
                // We don't and won't do any conversion from UTF-8 to
                // whatever STRING represents; let's just assume that
                // the other end is going to handle it correctly.
                if let Some(text) = self.copy_and_paste.clipboard(clipboard) {
                    conn.send_request_no_reply(&xcb::x::ChangeProperty {
                        mode: PropMode::Replace,
                        window: request.requestor(),
                        property: request.property(),
                        r#type: request.target(),
                        data: text.as_bytes(),
                    })?;
                    // let the requestor know that we set their property
                    request.property()
                } else {
                    // We have no clipboard so there is nothing to report
                    xcb::x::ATOM_NONE
                }
            } else {
                xcb::x::ATOM_NONE
            }
        } else {
            // We didn't support their request, so there is nothing
            // we can report back to them.
            xcb::x::ATOM_NONE
        };
        log::trace!(
            "SEL: window_id={window_id:?} responding with selprop={:?}",
            selprop
        );

        conn.send_request_no_reply(&xcb::x::SendEvent {
            propagate: true,
            destination: xcb::x::SendEventDest::Window(request.requestor()),
            event_mask: xcb::x::EventMask::empty(),
            event: &xcb::x::SelectionNotifyEvent::new(
                request.time(),
                request.requestor(),
                request.selection(),
                request.target(),
                selprop, // the disposition from the operation above
            ),
        })?;

        Ok(())
    }

    fn selection_notify(&mut self, selection: &xcb::x::SelectionNotifyEvent) -> anyhow::Result<()> {
        let conn = self.conn();
        let window_id = self.window_id;
        let selection_name = conn.atom_name(selection.selection());
        let target_name = conn.atom_name(selection.target());

        log::trace!(
            "SEL: window_id={window_id:?} SELECTION_NOTIFY received {selection:?} \
            selection.selection={selection_name} selection.target={target_name}"
        );

        if let Some(clipboard) = self.selection_atom_to_clipboard(selection.selection()) {
            if selection.property() == xcb::x::ATOM_NONE {
                if selection.target() == conn.atom_utf8_string {
                    log::trace!(
                        "SEL: window_id={window_id:?} -> UTF-8 selection data \
                         available, requesting STRING instead"
                    );
                    conn.send_request_no_reply_log(&xcb::x::ConvertSelection {
                        requestor: window_id,
                        selection: selection.selection(),
                        target: xcb::x::ATOM_STRING,
                        property: conn.atom_xsel_data,
                        time: self.copy_and_paste.time,
                    });
                    return Ok(());
                }

                if let Some(mut promise) = self.copy_and_paste.request_mut(clipboard).take() {
                    log::trace!(
                        "SEL: window_id={window_id:?} -> no compatible selection data \
                         available, fulfil promise with empty string"
                    );
                    promise.ok("".to_owned());
                    return Ok(());
                }
                log::trace!(
                    "SEL: window_id={window_id:?} -> no compatible selection data \
                     available, and no promise. weird!"
                );

                return Ok(());
            }

            match conn.send_and_wait_request(&xcb::x::GetProperty {
                delete: false,
                window: selection.requestor(),
                property: selection.property(),
                r#type: selection.target(),
                long_offset: 0,
                long_length: u32::max_value(),
            }) {
                Ok(prop) => {
                    if let Some(mut promise) = self.copy_and_paste.request_mut(clipboard).take() {
                        fn latin1_to_string(s: &[u8]) -> String {
                            s.iter().map(|&c| c as char).collect()
                        }

                        let data = if selection.target() == xcb::x::ATOM_STRING {
                            latin1_to_string(prop.value())
                        } else {
                            // selection.target() is probably == conn.atom_utf8_string,
                            // because we only ever ask for either STRING or UTF8_STRING.
                            // If it isn't, we'll just try to convert it anyway.
                            String::from_utf8_lossy(prop.value()).to_string()
                        };

                        promise.ok(data);
                    }

                    conn.send_request_no_reply(&xcb::x::DeleteProperty {
                        window: self.window_id,
                        property: conn.atom_xsel_data,
                    })?;
                }
                Err(err) => {
                    log::error!("clipboard: err while getting clipboard property: {:?}", err);
                    if let Some(mut promise) = self.copy_and_paste.request_mut(clipboard).take() {
                        promise.ok("".to_owned());
                    }
                }
            }
        } else if selection.selection() == conn.atom_xdndselection
            && selection.property() == conn.atom_xsel_data
        {
            if let Some(srcwin) = self.drag_and_drop.src_window {
                match conn.send_and_wait_request(&xcb::x::GetProperty {
                    delete: true,
                    window: selection.requestor(),
                    property: selection.property(),
                    r#type: selection.target(),
                    long_offset: 0,
                    long_length: u32::max_value(),
                }) {
                    Ok(prop) => {
                        if selection.target() == conn.atom_utf8_string {
                            let text = String::from_utf8_lossy(prop.value()).to_string();
                            self.events.dispatch(WindowEvent::DroppedString(text));
                        } else if selection.target() == conn.atom_xmozurl {
                            let data = decode_dropped_url_string(prop.value());
                            let urls = parse_xmozurl_list(&data);
                            self.events.dispatch(WindowEvent::DroppedUrl(urls));
                        } else if selection.target() == conn.atom_texturilist {
                            let paths = parse_texturi_list(prop.value());
                            self.events.dispatch(WindowEvent::DroppedFile(paths));
                        }
                    }
                    Err(err) => {
                        log::error!("clipboard: err while getting clipboard property: {err:#}");
                    }
                }
                conn.send_request_no_reply_log(&xcb::x::SendEvent {
                    propagate: false,
                    destination: xcb::x::SendEventDest::Window(srcwin),
                    event_mask: xcb::x::EventMask::empty(),
                    event: &xcb::x::ClientMessageEvent::new(
                        srcwin,
                        conn.atom_xdndfinished,
                        xcb::x::ClientMessageData::Data32([
                            window_id.resource_id(),
                            1,
                            self.drag_and_drop.target_action.resource_id(),
                            0,
                            0,
                        ]),
                    ),
                });
            } else {
                log::warn!("No Xdnd in progress, but received Xdnd selection. Ignoring.");
            }
        } else {
            log::trace!("SEL: window_id={window_id:?} unknown selection {selection_name}");
        }
        Ok(())
    }

    fn get_window_state(&self) -> anyhow::Result<WindowState> {
        let conn = self.conn();

        let reply = conn.send_and_wait_request(&xcb::x::GetProperty {
            delete: false,
            window: self.window_id,
            property: conn.atom_net_wm_state,
            r#type: xcb::x::ATOM_ATOM,
            long_offset: 0,
            long_length: 1024,
        })?;

        let state = reply.value::<u32>();
        let mut window_state = WindowState::default();

        let (mut vert, mut horz) = (false, false);
        for &s in state {
            if s == conn.atom_state_fullscreen.resource_id() {
                window_state |= WindowState::FULL_SCREEN;
            } else if s == conn.atom_state_maximized_vert.resource_id() {
                vert = true;
            } else if s == conn.atom_state_maximized_horz.resource_id() {
                horz = true;
            } else if s == conn.atom_state_hidden.resource_id() {
                window_state |= WindowState::HIDDEN;
            }
        }
        // both ways is maximized; one way is tiled (Chrome's reading of
        // the two hints)
        if vert && horz {
            window_state |= WindowState::MAXIMIZED;
        } else if vert || horz {
            window_state |= WindowState::TILED;
        }
        if let Some(edge) = self.edge.as_ref().filter(|_| Self::restored(window_state)) {
            window_state |= WindowState::CLIENT_EDGE;
            if edge.is_solid() {
                window_state |= WindowState::SOLID_EDGE;
            }
        }

        Ok(window_state)
    }

    fn set_wm_state(
        &mut self,
        action: NetWmStateAction,
        atom: Atom,
        atom2: Option<Atom>,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        let data: [u32; 5] = [
            action as u32,
            atom.resource_id(),
            atom2.map(|a| a.resource_id()).unwrap_or(0),
            0,
            0,
        ];

        // Ask window manager to change our fullscreen state
        conn.send_request_no_reply(&xcb::x::SendEvent {
            propagate: true,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                conn.atom_net_wm_state,
                xcb::x::ClientMessageData::Data32(data),
            ),
        })?;
        conn.flush()?;
        self.adjust_decorations(self.config.window_decorations)?;

        Ok(())
    }

    fn set_maximized_hint(&mut self, enable: bool) -> anyhow::Result<()> {
        self.set_wm_state(
            NetWmStateAction::with_bool(enable),
            self.conn().atom_state_maximized_vert,
            Some(self.conn().atom_state_maximized_horz),
        )
    }

    fn set_fullscreen_hint(&mut self, enable: bool) -> anyhow::Result<()> {
        self.set_wm_state(
            NetWmStateAction::with_bool(enable),
            self.conn().atom_state_fullscreen,
            None,
        )
    }

    #[allow(clippy::identity_op)]
    fn adjust_decorations(&mut self, decorations: WindowDecorations) -> anyhow::Result<()> {
        // Set the motif hints to disable decorations.
        // See https://stackoverflow.com/a/1909708
        #[repr(C)]
        struct MwmHints {
            flags: u32,
            functions: u32,
            decorations: u32,
            input_mode: i32,
            status: u32,
        }

        const HINTS_DECORATIONS: u32 = 1 << 1;
        const FUNC_ALL: u32 = 1 << 0;
        const FUNC_RESIZE: u32 = 1 << 1;
        // const HINTS_FUNCTIONS: u32 = 1 << 0;
        const FUNC_MOVE: u32 = 1 << 2;
        const FUNC_MINIMIZE: u32 = 1 << 3;
        const FUNC_MAXIMIZE: u32 = 1 << 4;
        const FUNC_CLOSE: u32 = 1 << 5;

        let decorations = if decorations == WindowDecorations::TITLE | WindowDecorations::RESIZE {
            FUNC_ALL
        } else if decorations.contains(WindowDecorations::INTEGRATED_BUTTONS) {
            // no decorations at all: KWin draws its whole frame, title
            // included, for anything else (a border alone too); the window
            // resizes from its own edges (`request_drag_resize`)
            0
        } else if decorations == WindowDecorations::RESIZE {
            FUNC_RESIZE
        } else if decorations == WindowDecorations::TITLE {
            FUNC_MOVE | FUNC_MINIMIZE | FUNC_MAXIMIZE | FUNC_CLOSE
        } else if decorations == WindowDecorations::NONE {
            0
        } else {
            FUNC_ALL
        };

        let hints = MwmHints {
            flags: HINTS_DECORATIONS,
            functions: 0,
            decorations,
            input_mode: 0,
            status: 0,
        };

        let conn = self.conn();

        let hints_slice =
            unsafe { std::slice::from_raw_parts(&hints as *const _ as *const u32, 5) };

        conn.send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: self.window_id,
            property: conn.atom_motif_wm_hints,
            r#type: conn.atom_motif_wm_hints,
            data: hints_slice,
        })?;
        Ok(())
    }

    fn conn(&self) -> Rc<XConnection> {
        self.conn.upgrade().expect("XConnection to be alive")
    }
}

/// A Window!
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct XWindow(xcb::x::Window);

impl PartialOrd for XWindow {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.resource_id().partial_cmp(&other.0.resource_id())
    }
}

impl Ord for XWindow {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.resource_id().cmp(&other.0.resource_id())
    }
}

impl XWindow {
    pub(crate) fn from_id(id: xcb::x::Window) -> Self {
        Self(id)
    }

    /// Create a new window on the specified screen with the specified
    /// dimensions
    pub async fn new_window<F>(
        class_name: &str,
        name: &str,
        geometry: RequestedWindowGeometry,
        config: Option<&ConfigHandle>,
        _font_config: Rc<FontConfiguration>,
        event_handler: F,
    ) -> anyhow::Result<Window>
    where
        F: 'static + FnMut(WindowEvent, &Window),
    {
        let config = match config {
            Some(c) => c.clone(),
            None => config::configuration(),
        };
        let conn = Connection::get()
            .ok_or_else(|| {
                anyhow!(
                "new_window must be called on the gui thread after Connection::init has succeeded",
            )
            })?
            .x11();

        let ResolvedGeometry {
            x,
            y,
            width,
            height,
        } = conn.resolve_geometry(geometry);

        let mut events = WindowEventSender::new(event_handler);

        // the window's own edge: the desktop theme's, where the window
        // manager and a compositor let the shadow hang outside the
        // window; Chrome's own frame anywhere (with a shadow under those
        // conditions, else solid; round corners with a compositor)
        let edge = config
            .integrated_window_edge
            .as_ref()
            .filter(|c| {
                config
                    .window_decorations
                    .contains(WindowDecorations::INTEGRATED_BUTTONS)
                    && (c.chrome || conn.supports_client_side_edge())
            })
            .and_then(|c| {
                let compositor = conn.has_compositor();
                let shadow = compositor && conn.wm_takes_frame_extents();
                match crate::os::edge::Edge::new(c, shadow, compositor) {
                    Ok(edge) => Some(edge),
                    Err(err) => {
                        log::warn!("window edge: {err:#}");
                        None
                    }
                }
            });
        let insets = edge
            .as_ref()
            .map(|e| e.insets(conn.default_dpi() / 96., true))
            .unwrap_or_default();
        let outer_width = width + usize::from(insets.left + insets.right);
        let outer_height = height + usize::from(insets.top + insets.bottom);

        let window_id;
        let child_id;
        let window = {
            let setup = conn.conn().get_setup();
            let screen = setup
                .roots()
                .nth(conn.screen_num() as usize)
                .ok_or_else(|| anyhow!("no screen?"))?;

            window_id = conn.conn().generate_id();
            child_id = conn.conn().generate_id();

            let color_map_id = conn.conn().generate_id();
            conn.send_request_no_reply(&xcb::x::CreateColormap {
                alloc: xcb::x::ColormapAlloc::None,
                mid: color_map_id,
                window: screen.root(),
                visual: conn.visual.visual_id(),
            })
            .context("create_colormap_checked")?;

            conn.send_request_no_reply(&xcb::x::CreateWindow {
                depth: conn.depth,
                wid: window_id,
                parent: screen.root(),
                x: x.unwrap_or(0).try_into()?,
                y: y.unwrap_or(0).try_into()?,
                width: outer_width.try_into()?,
                height: outer_height.try_into()?,
                border_width: 0,
                class: xcb::x::WindowClass::InputOutput,
                visual: conn.visual.visual_id(),
                value_list: &[
                    // We have to specify both a border pixel color and a colormap
                    // when specifying a depth that doesn't match the root window in
                    // order to avoid a BadMatch
                    xcb::x::Cw::BackPixel(0), // transparent background
                    xcb::x::Cw::BorderPixel(screen.black_pixel()),
                    xcb::x::Cw::EventMask(
                        xcb::x::EventMask::FOCUS_CHANGE
                            | xcb::x::EventMask::KEY_PRESS
                            | xcb::x::EventMask::BUTTON_PRESS
                            | xcb::x::EventMask::BUTTON_RELEASE
                            | xcb::x::EventMask::POINTER_MOTION
                            | xcb::x::EventMask::LEAVE_WINDOW
                            | xcb::x::EventMask::BUTTON_MOTION
                            | xcb::x::EventMask::KEY_RELEASE
                            | xcb::x::EventMask::PROPERTY_CHANGE
                            | xcb::x::EventMask::STRUCTURE_NOTIFY
                            // the margins, painted by us
                            | xcb::x::EventMask::EXPOSURE,
                    ),
                    xcb::x::Cw::Colormap(color_map_id),
                ],
            })
            .context("xcb::create_window_checked")?;

            conn.send_request_no_reply(&xcb::x::CreateWindow {
                depth: conn.depth,
                wid: child_id,
                parent: window_id,
                x: insets.left as i16,
                y: insets.top as i16,
                width: width.try_into()?,
                height: height.try_into()?,
                border_width: 0,
                class: xcb::x::WindowClass::InputOutput,
                visual: conn.visual.visual_id(),
                value_list: &[
                    // We have to specify both a border pixel color and a colormap
                    // when specifying a depth that doesn't match the root window in
                    // order to avoid a BadMatch
                    xcb::x::Cw::BackPixel(0), // transparent background
                    xcb::x::Cw::BorderPixel(screen.black_pixel()),
                    xcb::x::Cw::BitGravity(xcb::x::Gravity::NorthWest),
                    xcb::x::Cw::EventMask(xcb::x::EventMask::EXPOSURE),
                    xcb::x::Cw::Colormap(color_map_id),
                ],
            })
            .context("xcb::create_window_checked")?;

            conn.send_request_no_reply(&xcb::x::MapWindow { window: child_id })
                .context("xcb::map_window")?;

            events.assign_window(Window::X11(XWindow::from_id(window_id)));

            let appearance = conn.get_appearance();

            Arc::new(Mutex::new(XWindowInner {
                title: String::new(),
                appearance,
                window_id,
                child_id,
                conn: Rc::downgrade(&conn),
                events,
                width: width.try_into()?,
                height: height.try_into()?,
                dpi: conn.default_dpi(),
                copy_and_paste: CopyAndPaste::default(),
                drag_and_drop: DragAndDrop::default(),
                cursors: CursorInfo::new(&config, &conn),
                config: config.clone(),
                has_focus: None,
                verify_focus: true,
                last_cursor_position: Rect::default(),
                paint_throttled: false,
                last_wm_state: WindowState::default(),
                invalidated: false,
                pending: vec![],
                sure_about_geometry: false,
                current_mouse_event: None,
                window_drag_position: None,
                dragging: false,
                outstanding_configure_requests: 0,
                pending_finished_resizes: 0,
                edge,
                insets,
                outer: (outer_width.try_into()?, outer_height.try_into()?),
                edge_painted: None,
                fitted: None,
                tiled: false,
                edge_gc: None,
                edge_pixmaps: vec![],
                gui_cursor: None,
                on_edge: false,
            }))
        };

        // WM_CLASS is encoded as the class and instance name,
        // null terminated
        let mut class_string = class_name.as_bytes().to_vec();
        class_string.push(0);
        class_string.extend_from_slice(class_name.as_bytes());
        class_string.push(0);

        conn.send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: window_id,
            property: xcb::x::ATOM_WM_CLASS,
            r#type: xcb::x::ATOM_STRING,
            data: &class_string,
        })?;

        conn.send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: window_id,
            property: conn.atom_net_wm_pid,
            r#type: xcb::x::ATOM_CARDINAL,
            data: &[unsafe { libc::getpid() as u32 }],
        })?;

        conn.send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: window_id,
            property: conn.atom_protocols,
            r#type: xcb::x::ATOM_ATOM,
            data: &[conn.atom_delete],
        })?;

        conn.send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: window_id,
            property: conn.atom_xdndaware,
            r#type: xcb::x::ATOM_ATOM,
            data: &[5u32],
        })?;

        window
            .lock()
            .unwrap()
            .adjust_decorations(config.window_decorations)?;

        let window_handle = Window::X11(XWindow::from_id(window_id));

        conn.windows.borrow_mut().insert(window_id, window);
        conn.child_to_parent_id
            .borrow_mut()
            .insert(child_id, window_id);

        window_handle.set_title(name);
        // Before we map the window, flush to ensure that all of the other properties
        // have been applied to it.
        // This is a speculative fix for this race condition issue:
        // <https://github.com/wezterm/wezterm/issues/2155>
        conn.flush().context("flushing before mapping window")?;
        window_handle.show();

        // Some window managers will ignore the x,y that we set during window
        // creation, so we ask them again once the window is mapped
        if let (Some(x), Some(y)) = (x, y) {
            window_handle.set_window_position(ScreenPoint::new(x.try_into()?, y.try_into()?));
        }

        if conn
            .active_extensions()
            .any(|e| e == xcb::Extension::Present)
        {
            let event_id = conn.generate_id();
            conn.send_request_no_reply(&xcb::present::SelectInput {
                eid: event_id,
                window: window_id,
                event_mask: xcb::present::EventMask::CONFIGURE_NOTIFY,
            })
            .context("Present::SelectInput")?;
        }

        Ok(window_handle)
    }
}

impl XWindowInner {
    fn close(&mut self) {
        let conn = self.conn();
        conn.flush()
            .context("flush pending requests prior to issuing DestroyWindow")
            .ok();
        // Remove the window from the map now, as GL state
        // requires that it is able to make_current() in its
        // Drop impl, and that cannot succeed after we've
        // destroyed the window at the X11 level.
        self.conn().windows.borrow_mut().remove(&self.window_id);
        self.conn()
            .child_to_parent_id
            .borrow_mut()
            .remove(&self.child_id);

        // Unmap the window first: calling DestroyWindow here may race
        // with some requests made either by EGL or the IME, but I haven't
        // been able to pin down the source.
        // We'll destroy the window in a couple of seconds
        conn.send_request_no_reply_log(&xcb::x::UnmapWindow {
            window: self.window_id,
        });

        conn.send_request_no_reply_log(&xcb::x::DestroyWindow {
            window: self.child_id,
        });

        // Arrange to destroy the window after a couple of seconds; that
        // should give whatever stuff is still referencing the window
        // to finish and avoid triggering a protocol error.
        // I don't really like this as a solution :-/
        // <https://github.com/wezterm/wezterm/issues/2198>
        let window = self.window_id;
        promise::spawn::spawn(async move {
            async_io::Timer::after(std::time::Duration::from_secs(2)).await;
            let conn = Connection::get().unwrap().x11();
            log::trace!("close sending DestroyWindow for {:?}", window);
            conn.send_request_no_reply_log(&xcb::x::DestroyWindow { window });
        })
        .detach();
        // Ensure that we don't try to destroy the window twice,
        // otherwise the rust xcb bindings will generate a
        // fatal error!
        log::trace!("clear out self.window_id");
        self.window_id = xcb::x::Window::none();
    }
    /// Minimize: ask the window manager to iconify the window, the
    /// ICCCM way (what Xlib's XIconifyWindow sends)
    fn hide(&mut self) {
        const ICONIC_STATE: u32 = 3;
        let conn = self.conn();
        conn.send_request_no_reply_log(&xcb::x::SendEvent {
            propagate: false,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                conn.atom_wm_change_state,
                xcb::x::ClientMessageData::Data32([ICONIC_STATE, 0, 0, 0, 0]),
            ),
        });

        if let Err(err) = conn.flush() {
            log::error!("Error flushing: {err:#}");
        }
    }
    fn show(&mut self) {
        self.conn().send_request_no_reply_log(&xcb::x::MapWindow {
            window: self.window_id,
        });
    }

    fn focus(&mut self) {
        let conn = self.conn();
        conn.send_request_no_reply_log(&xcb::x::SendEvent {
            propagate: true,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                conn.atom_net_active_window,
                xcb::x::ClientMessageData::Data32([
                    1,
                    // You'd think that self.copy_and_paste.time would
                    // be the thing to use, but Mutter ignored this request
                    // until I switched to CURRENT_TIME
                    xcb::x::CURRENT_TIME,
                    0,
                    0,
                    0,
                ]),
            ),
        });

        if let Err(err) = conn.flush() {
            log::error!("Error flushing: {err:#}");
        }
    }

    fn invalidate(&mut self) {
        self.queue_pending(WindowEvent::NeedRepaint);
        self.dispatch_pending_events().ok();
    }

    fn maximize(&mut self) {
        if let Err(err) = self.set_maximized_hint(true) {
            log::error!("Failed to maximize: {err:#}");
        }
    }

    fn restore(&mut self) {
        if let Err(err) = self.set_maximized_hint(false) {
            log::error!("Failed to restore: {err:#}");
        }
    }

    fn toggle_fullscreen(&mut self) {
        let fullscreen = match self.get_window_state() {
            Ok(f) => f.contains(WindowState::FULL_SCREEN),
            Err(err) => {
                log::error!("Failed to determine fullscreen state: {}", err);
                return;
            }
        };
        self.set_fullscreen_hint(!fullscreen).ok();
    }

    fn config_did_change(&mut self, config: &ConfigHandle) {
        let dpi_changed =
            self.config.dpi != config.dpi || self.config.dpi_by_screen != config.dpi_by_screen;
        self.config = config.clone();
        let _ = self.adjust_decorations(config.window_decorations);

        if dpi_changed {
            let _ = self.configure_notify("config reload", self.outer.0, self.outer.1);
        }
    }

    fn net_wm_moveresize(&mut self, x_root: u32, y_root: u32, direction: u32, button: u32) {
        let source_indication = 1;
        let conn = self.conn();

        if !conn
            .supported
            .borrow()
            .contains(&conn.atom_net_wm_moveresize)
        {
            log::debug!("WM doesn't support _NET_WM_MOVERESIZE");
            return;
        }

        log::debug!("net_wm_moveresize {x_root},{y_root} direction={direction} button={button}");

        if direction != _NET_WM_MOVERESIZE_CANCEL {
            // Tell the server to ungrab. Even though we haven't explicitly
            // grabbed it in our application code, there's an implicit grab
            // as part of a mouse drag and the moveresize will do nothing
            // if we don't ungrab it.
            conn.send_request_no_reply_log(&xcb::x::UngrabPointer {
                time: self.copy_and_paste.time,
            });
            // Flag to ourselves that we are dragging.
            // This is also used to gate the fallback of calling
            // set_window_position in case the WM doesn't support
            // _NET_WM_MOVERESIZE and we returned early above.
            self.dragging = true;
        }

        conn.send_request_no_reply_log(&xcb::x::SendEvent {
            propagate: true,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                conn.atom_net_wm_moveresize,
                xcb::x::ClientMessageData::Data32([
                    x_root,
                    y_root,
                    direction,
                    button,
                    source_indication,
                ]),
            ),
        });
        conn.flush().context("flush moveresize").ok();
    }

    fn request_drag_move(&mut self) -> anyhow::Result<()> {
        let pos = self.window_drag_position.unwrap_or_default();

        let x_root = pos.x as u32;
        let y_root = pos.y as u32;
        let button = 1; // Left

        self.net_wm_moveresize(x_root, y_root, _NET_WM_MOVERESIZE_MOVE, button);
        Ok(())
    }

    /// Resizing from `edge` by the window manager, as a drag from where
    /// the pointer went down (the edges are the window's own: it asked
    /// for no frame, see `adjust_decorations`).
    fn request_drag_resize(&mut self, edge: crate::ResizeEdge) -> anyhow::Result<()> {
        let pos = self.window_drag_position.unwrap_or_default();
        // ResizeEdge is in the order of _NET_WM_MOVERESIZE_SIZE_TOPLEFT (0)
        // to _NET_WM_MOVERESIZE_SIZE_LEFT (7)
        self.net_wm_moveresize(pos.x as u32, pos.y as u32, edge as u32, 1);
        Ok(())
    }

    /// Beneath the other windows (Chrome's X11Window::LowerWindow: the
    /// window restacked below its siblings, the window manager deciding)
    fn lower(&mut self) {
        self.conn()
            .send_request_no_reply_log(&xcb::x::ConfigureWindow {
                window: self.window_id,
                value_list: &[xcb::x::ConfigWindow::StackMode(xcb::x::StackMode::Below)],
            });
    }

    /// The window manager's window menu at `screen` (GTK's
    /// gdk_x11_window_show_window_menu: the pointer's implicit grab given
    /// up, `_GTK_SHOW_WINDOW_MENU` sent to the root with the device, 0 for
    /// the core pointer, and the root coordinates)
    fn show_window_menu(&mut self, screen: ScreenPoint) {
        let conn = self.conn();
        let cookie = conn.conn().send_request(&xcb::x::InternAtom {
            only_if_exists: false,
            name: b"_GTK_SHOW_WINDOW_MENU",
        });
        let Ok(atom) = conn.conn().wait_for_reply(cookie).map(|r| r.atom()) else {
            return;
        };
        if !conn.supported.borrow().contains(&atom) {
            log::debug!("the window manager has no _GTK_SHOW_WINDOW_MENU");
            return;
        }
        conn.send_request_no_reply_log(&xcb::x::UngrabPointer {
            time: self.copy_and_paste.time,
        });
        conn.send_request_no_reply_log(&xcb::x::SendEvent {
            propagate: false,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                atom,
                xcb::x::ClientMessageData::Data32([0, screen.x as u32, screen.y as u32, 0, 0]),
            ),
        });
        let _ = conn.flush();
    }

    fn set_window_position(&mut self, coords: ScreenPoint) {
        if self.dragging {
            return;
        }

        // We ask the window manager to move the window for us so that
        // we don't have to deal with adjusting for the frame size.
        // Note that neither this technique or the configure_window
        // approach below will successfully move a window running
        // under the crostini environment on a chromebook :-(
        let conn = self.conn();

        conn.send_request_no_reply_log(&xcb::x::SendEvent {
            propagate: true,
            destination: xcb::x::SendEventDest::Window(conn.root),
            event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
            event: &xcb::x::ClientMessageEvent::new(
                self.window_id,
                conn.atom_net_move_resize_window,
                xcb::x::ClientMessageData::Data32([
                    xcb::x::Gravity::Static as u32 |
            1<<12 | // normal program
            xcb_util::MOVE_RESIZE_MOVE
                | xcb_util::MOVE_RESIZE_WINDOW_X
                | xcb_util::MOVE_RESIZE_WINDOW_Y,
                    coords.x as u32,
                    coords.y as u32,
                    self.width as u32,
                    self.height as u32,
                ]),
            ),
        });
    }

    /// Change the title for the window manager
    fn set_title(&mut self, title: &str) {
        if title == self.title {
            return;
        }
        self.title = title.to_string();

        let conn = self.conn();

        conn.send_request_no_reply_log(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: self.window_id,
            property: xcb::x::ATOM_WM_NAME,
            r#type: conn.atom_utf8_string,
            data: title.as_bytes(),
        });

        // Also set EWMH _NET_WM_NAME, as some clients don't correctly
        // fall back to reading WM_NAME
        conn.send_request_no_reply_log(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: self.window_id,
            property: conn.atom_net_wm_name,
            r#type: conn.atom_utf8_string,
            data: title.as_bytes(),
        });
    }

    fn set_text_cursor_position(&mut self, cursor: Rect) {
        if self.last_cursor_position == cursor {
            return;
        }
        self.last_cursor_position = cursor;
        self.update_ime_position();
    }

    fn update_ime_position(&mut self) {
        if !self.has_focus.unwrap_or(false) {
            return;
        }
        self.conn().ime.borrow_mut().update_pos(
            self.window_id,
            self.last_cursor_position.min_x() as i16,
            self.last_cursor_position.max_y() as i16,
        );
    }

    fn set_icon(&mut self, image: &dyn BitmapImage) {
        let (width, height) = image.image_dimensions();

        // https://specifications.freedesktop.org/wm-spec/wm-spec-1.3.html#idm44927025355360
        // says that this is an array of 32bit ARGB data.
        // The first two elements are width, height, with the remainder
        // being the row data, left-to-right, top-to-bottom.
        let mut icon_data = Vec::with_capacity((2 + (width * height)) * 4);
        icon_data.push(width as u32);
        icon_data.push(height as u32);
        // `BitmapImage` is rgba32, so we need to munge to get argb32.
        // We also need to put the data into big endian format.
        for pixel in image.pixels() {
            let [r, g, b, a] = pixel.to_ne_bytes();
            icon_data.push(u32::from_be_bytes([a, r, g, b]));
        }

        self.conn()
            .send_request_no_reply_log(&xcb::x::ChangeProperty {
                mode: PropMode::Replace,
                window: self.window_id,
                property: self.conn().atom_net_wm_icon,
                r#type: xcb::x::ATOM_CARDINAL,
                data: &icon_data,
            });
    }

    fn set_resize_increments(&mut self, incr: ResizeIncrement) -> anyhow::Result<()> {
        use xcb_util::*;
        let hints = xcb_size_hints_t {
            flags: XCB_ICCCM_SIZE_HINT_P_MIN_SIZE
                | XCB_ICCCM_SIZE_HINT_P_RESIZE_INC
                | XCB_ICCCM_SIZE_HINT_BASE_SIZE,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            min_width: (incr.base_width + incr.x).into(),
            min_height: (incr.base_height + incr.y).into(),
            max_width: 0,
            max_height: 0,
            width_inc: incr.x.into(),
            height_inc: incr.y.into(),
            min_aspect_num: 0,
            min_aspect_den: 0,
            max_aspect_num: 0,
            max_aspect_den: 0,
            base_width: incr.base_width.into(),
            base_height: incr.base_height.into(),
            win_gravity: 0,
        };

        let data = unsafe {
            std::slice::from_raw_parts(
                &hints as *const _ as *const u32,
                std::mem::size_of::<xcb_size_hints_t>() / 4,
            )
        };

        self.conn().send_request_no_reply(&xcb::x::ChangeProperty {
            mode: PropMode::Replace,
            window: self.window_id,
            property: xcb::x::ATOM_WM_NORMAL_HINTS,
            r#type: xcb::x::ATOM_WM_SIZE_HINTS,
            data,
        })?;

        Ok(())
    }
}

impl HasDisplayHandle for XWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let conn = Connection::get()
            .expect("display_handle only callable on main thread")
            .x11();
        let handle = XcbDisplayHandle::new(NonNull::new(conn.get_raw_conn() as _), conn.screen_num);

        unsafe { Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Xcb(handle))) }
    }
}

impl HasWindowHandle for XWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let conn = Connection::get().expect("window_handle only callable on main thread");
        let handle = conn
            .x11()
            .window_by_id(self.0)
            .expect("window handle invalid!?");

        let inner = handle.lock().unwrap();
        let handle = inner.window_handle()?;
        unsafe { Ok(WindowHandle::borrow_raw(handle.as_raw())) }
    }
}

#[async_trait(?Send)]
impl WindowOps for XWindow {
    async fn enable_opengl(&self) -> anyhow::Result<Rc<glium::backend::Context>> {
        let window = self.0;
        promise::spawn::spawn(async move {
            if let Some(handle) = Connection::get().unwrap().x11().window_by_id(window) {
                let mut inner = handle.lock().unwrap();
                inner.enable_opengl()
            } else {
                anyhow::bail!("invalid window");
            }
        })
        .await
    }

    fn notify<T: Any + Send + Sync>(&self, t: T)
    where
        Self: Sized,
    {
        XConnection::with_window_inner(self.0, move |inner| {
            inner
                .events
                .dispatch(WindowEvent::Notification(Box::new(t)));
            Ok(())
        });
    }

    fn close(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.close();
            Ok(())
        });
    }

    fn hide(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.hide();
            Ok(())
        });
    }

    fn toggle_fullscreen(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.toggle_fullscreen();
            Ok(())
        });
    }

    fn maximize(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.maximize();
            Ok(())
        });
    }

    fn restore(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.restore();
            Ok(())
        });
    }

    fn config_did_change(&self, config: &ConfigHandle) {
        let config = config.clone();
        XConnection::with_window_inner(self.0, move |inner| {
            inner.config_did_change(&config);
            Ok(())
        });
    }

    fn focus(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.focus();
            Ok(())
        });
    }

    fn show(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.show();
            Ok(())
        });
    }

    fn set_cursor(&self, cursor: Option<CursorIcon>) {
        XConnection::with_window_inner(self.0, move |inner| {
            let _ = inner.set_cursor(cursor);
            Ok(())
        });
    }

    fn invalidate(&self) {
        XConnection::with_window_inner(self.0, |inner| {
            inner.invalidate();
            Ok(())
        });
    }

    fn set_title(&self, title: &str) {
        let title = title.to_owned();
        XConnection::with_window_inner(self.0, move |inner| {
            inner.set_title(&title);
            Ok(())
        });
    }

    fn set_inner_size(&self, width: usize, height: usize) {
        XConnection::with_window_inner(self.0, move |inner| {
            let i = inner.insets;
            inner
                .conn()
                .send_request_no_reply_log(&xcb::x::ConfigureWindow {
                    window: inner.window_id,
                    value_list: &[
                        xcb::x::ConfigWindow::Width((width + usize::from(i.left + i.right)) as u32),
                        xcb::x::ConfigWindow::Height(
                            (height + usize::from(i.top + i.bottom)) as u32,
                        ),
                    ],
                });
            inner.resize_child(width as u32, height as u32);
            inner.outstanding_configure_requests += 1;
            Ok(())
        });
    }

    fn request_drag_move(&self) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.request_drag_move()?;
            Ok(())
        });
    }

    /// X11 draws no frame for a window whose decorations integrate the
    /// buttons (see `adjust_decorations`); the caller knows its config.
    fn resizes_from_own_edges(&self) -> bool {
        true
    }

    fn request_drag_resize(&self, edge: crate::ResizeEdge) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.request_drag_resize(edge)?;
            Ok(())
        });
    }

    fn lower(&self) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.lower();
            Ok(())
        });
    }

    fn show_window_menu(&self, _coords: Point, screen_coords: ScreenPoint) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.show_window_menu(screen_coords);
            Ok(())
        });
    }

    fn set_window_drag_position(&self, coords: ScreenPoint) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.window_drag_position.replace(coords);
            Ok(())
        });
    }

    fn set_window_position(&self, coords: ScreenPoint) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.set_window_position(coords);
            Ok(())
        });
    }

    fn set_text_cursor_position(&self, cursor: Rect) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.set_text_cursor_position(cursor);
            Ok(())
        });
    }

    fn set_icon(&self, image: Image) {
        XConnection::with_window_inner(self.0, move |inner| {
            inner.set_icon(&image);
            Ok(())
        });
    }

    fn set_resize_increments(&self, incr: ResizeIncrement) {
        XConnection::with_window_inner(self.0, move |inner| {
            if let Err(err) = inner.set_resize_increments(incr) {
                log::error!("set_resize_increments failed: {:#}", err);
            }
            Ok(())
        });
    }

    /// Initiate textual transfer from the clipboard
    fn get_clipboard(&self, clipboard: Clipboard) -> Future<String> {
        let window_id = self.0;
        log::trace!("SEL: window_id={window_id:?} Window::get_clipboard {clipboard:?} called");
        let mut promise = Promise::new();
        let future = promise.get_future().unwrap();
        let mut promise = Some(promise);

        XConnection::with_window_inner(window_id, move |inner| {
            // In theory, we could simply consult inner.copy_and_paste to see
            // if we think we own the clipboard, but there are some situations
            // where the selection owner moves between two wezterm windows
            // where we don't receive a SELECTION_NOTIFY in time to correctly
            // invalidate that state, so we always ask the X server to for
            // the selection, even if it is a little slower.
            // <https://github.com/wezterm/wezterm/issues/2110>
            let promise = promise.take().unwrap();
            log::debug!(
                "SEL: window_id={window_id:?} Window::get_clipboard: \
                        {clipboard:?}, prepare promise, time={}",
                inner.copy_and_paste.time
            );
            inner.copy_and_paste.request_mut(clipboard).replace(promise);
            let conn = inner.conn();
            // Find the owner and ask them to send us the buffer
            conn.send_request_no_reply_log(&xcb::x::ConvertSelection {
                requestor: inner.window_id,
                selection: match clipboard {
                    Clipboard::Clipboard => conn.atom_clipboard,
                    Clipboard::PrimarySelection => xcb::x::ATOM_PRIMARY,
                },
                target: conn.atom_utf8_string,
                property: conn.atom_xsel_data,
                time: inner.copy_and_paste.time,
            });
            Ok(())
        });

        future
    }

    /// Set some text in the clipboard
    fn set_clipboard(&self, clipboard: Clipboard, text: String) {
        let window_id = self.0;
        XConnection::with_window_inner(window_id, move |inner| {
            log::trace!(
                "SEL: window_id={window_id:?} now owns selection \
                for {clipboard:?} {text:?}"
            );
            inner
                .copy_and_paste
                .clipboard_mut(clipboard)
                .replace(text.clone());
            inner.update_selection_owner(clipboard)?;
            Ok(())
        });
    }
}

fn parse_texturi_list(url_list: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(url_list)
        .lines()
        .filter_map(|line| {
            if line.starts_with('#') || line.trim().is_empty() {
                // text/uri-list: Any lines beginning with the '#' character
                // are comment lines and are ignored during processing
                return None;
            }
            let url = Url::parse(line)
                .map_err(|err| {
                    log::error!("Error parsing dropped file line {line} as url: {err:#}");
                })
                .ok()?;
            url.to_file_path()
                .map_err(|_| {
                    log::error!("Error converting url {url:?} from line {line} to pathbuf");
                })
                .ok()
        })
        .collect()
}

fn parse_xmozurl_list(url_list: &str) -> Vec<Url> {
    url_list
        .lines()
        .step_by(2)
        .filter_map(|line| {
            // the lines alternate between the urls and their titles
            Url::parse(line)
                .map_err(|err| {
                    log::error!("Error parsing dropped file line {line} as url: {err:#}");
                })
                .ok()
        })
        .collect()
}

/// Data may be UTF16 in either byte order, or UTF8
fn decode_dropped_url_string(raw: &[u8]) -> String {
    if raw.len() >= 2 && ((raw[0], raw[1]) == (0xfe, 0xff) || (raw[0] != 0x00 && raw[1] == 0x00)) {
        String::from_utf16_lossy(
            raw.chunks_exact(2)
                .map(|x: &[u8]| u16::from(x[1]) << 8 | u16::from(x[0]))
                .collect::<Vec<u16>>()
                .as_slice(),
        )
    } else if raw.len() >= 2
        && ((raw[0], raw[1]) == (0xff, 0xfe) || (raw[0] == 0x00 && raw[1] != 0x00))
    {
        String::from_utf16_lossy(
            raw.chunks_exact(2)
                .map(|x: &[u8]| u16::from(x[0]) << 8 | u16::from(x[1]))
                .collect::<Vec<u16>>()
                .as_slice(),
        )
    } else {
        String::from_utf8_lossy(raw).to_string()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[repr(u32)]
enum NetWmStateAction {
    Remove = 0,
    Add = 1,
    #[allow(dead_code)]
    Toggle = 2,
}

impl NetWmStateAction {
    fn with_bool(enable: bool) -> Self {
        if enable {
            Self::Add
        } else {
            Self::Remove
        }
    }
}
