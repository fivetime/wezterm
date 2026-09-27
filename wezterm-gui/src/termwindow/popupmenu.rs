//! A menu that pops up where the mouse is, drawn by the GUI over the
//! window as the command palette is (not in the pane, as `InputSelector`
//! is). Its lines come from a `PopupMenu` action — items, headings,
//! separators, each with an optional nerdfont icon, dimmed when disabled
//! — and the choice goes to the action's callback the way
//! `InputSelector`'s does: `(window, pane, id, label)`, nil for both when
//! the menu is dismissed.
//!
//! The mouse: it takes the window's mouse events while it is open
//! (`Modal::window_mouse_event`); the item under the pointer is
//! highlighted, a click on one chooses it, and a press anywhere outside
//! dismisses the menu. A right button still held from the click that
//! opened the menu chooses where it is released only after the pointer
//! moved, as desktop menus do. The keyboard: Up and Down move, Enter
//! chooses, Escape dismisses.

use crate::overlay::selector::trampoline;
use crate::scripting::guiwin::GuiWin;
use crate::termwindow::box_model::*;
use crate::termwindow::menuimage::{MenuColors, MenuLayout};
use crate::termwindow::modal::Modal;
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::{DimensionContext, TermWindow, TermWindowNotif, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::keyassignment::{InputSelectorEntry, KeyAssignment, PopupMenu as PopupMenuArgs};
use config::Dimension;
use mux_lua::MuxPane;
use std::cell::{Cell, Ref, RefCell};
use termwiz::nerdfonts::NERD_FONTS;
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};
use window::color::LinearRgba;
use window::{
    Connection, ConnectionOps, MouseEvent as WindowMouseEvent, MouseEventKind as WMEK, PopupEvent,
    RectF, Window, WindowOps,
};

/// How far the pointer must move before a held button counts as a drag
/// onto an item.
const DRAG_SLOP: f32 = 4.;

/// The space around an item's text, in cells: left, right, top/bottom.
const ITEM_PADDING: (f32, f32, f32) = (0.75, 1.5, 0.2);

pub struct PopupMenu {
    args: PopupMenuArgs,
    event_name: String,
    pane: MuxPane,
    /// Where it was opened, in window pixels.
    at: (f32, f32),
    /// The highlighted item (pointer or keyboard).
    selected: Cell<Option<usize>>,
    /// The item a button went down on inside the menu.
    pressed: Cell<Option<usize>>,
    /// The pointer moved away from where the menu opened.
    moved: Cell<bool>,
    /// Where it was last drawn: kept when the layout is thrown away (a
    /// reconfigure) until it is drawn again, so a click in between still
    /// counts as inside.
    bounds: Cell<Option<RectF>>,
    element: RefCell<Option<Vec<ComputedElement>>>,
    /// Shown in a window of its own (`open_popup`), where there are
    /// such: as the window's, and laid out as this.
    popup: RefCell<Option<(Window, MenuLayout)>>,
}

impl Drop for PopupMenu {
    fn drop(&mut self) {
        // however the menu went, its window goes with it
        if let Some((window, _)) = self.popup.borrow_mut().take() {
            window.close_popup();
        }
    }
}

impl PopupMenu {
    pub fn new(args: PopupMenuArgs, pane: MuxPane, at: (f32, f32)) -> anyhow::Result<Self> {
        let event_name = match *args.action {
            KeyAssignment::EmitEvent(ref id) => id.to_string(),
            _ => {
                anyhow::bail!("PopupMenu requires action to be defined by wezterm.action_callback")
            }
        };
        Ok(Self {
            args,
            event_name,
            pane,
            at,
            selected: Cell::new(None),
            pressed: Cell::new(None),
            moved: Cell::new(false),
            bounds: Cell::new(None),
            element: RefCell::new(None),
            popup: RefCell::new(None),
        })
    }

    fn layout(&self, term_window: &TermWindow, window: &Window) -> anyhow::Result<MenuLayout> {
        let font = term_window.fonts.title_font()?;
        let (bg, fg) = (
            *term_window.config.command_palette_bg_color,
            *term_window.config.command_palette_fg_color,
        );
        let translucent = Connection::get().is_some_and(|c| c.popups_translucent());
        // (fonts found later for what the title font has not: laid out
        // again then, see `reconfigure`)
        let notified = window.clone();
        let on_fonts = std::sync::Arc::new(move || {
            notified.notify(TermWindowNotif::Apply(Box::new(|term_window| {
                term_window.invalidate_modal();
            })));
        });
        Ok(MenuLayout::new(
            &self.args.choices,
            &font,
            MenuColors {
                bg: [bg.0, bg.1, bg.2],
                fg: [fg.0, fg.1, fg.2],
            },
            term_window.dimensions.dpi,
            translucent,
            on_fonts,
        ))
    }

    /// Shows the menu in a window of its own at the pointer, as Chrome's
    /// menus are (MenuHost), where the window system has such windows:
    /// it then reaches beyond this window. Drawn over this window
    /// otherwise, as before.
    pub fn open_popup(&self, term_window: &mut TermWindow) {
        let Some(window) = term_window.window.clone() else {
            return;
        };
        let layout = match self.layout(term_window, &window) {
            Ok(layout) => layout,
            Err(err) => {
                log::error!("PopupMenu: {err:#}");
                return;
            }
        };
        let notified = window.clone();
        let shown = window.show_popup(
            ::window::Point::new(self.at.0 as isize, self.at.1 as isize),
            layout.paint(self.selected.get()),
            Box::new(move |event| {
                notified.notify(TermWindowNotif::Apply(Box::new(move |term_window| {
                    term_window.popup_event(event)
                })));
            }),
        );
        if shown {
            self.popup.borrow_mut().replace((window, layout));
        }
    }

    /// The popup window's picture again, for another highlighted line.
    fn repaint(&self, term_window: &mut TermWindow) {
        match self.popup.borrow().as_ref() {
            Some((window, layout)) => window.update_popup(layout.paint(self.selected.get())),
            None => term_window.invalidate_modal(),
        }
    }

    fn choosable(&self, idx: usize) -> bool {
        self.args
            .choices
            .get(idx)
            .map_or(false, |c| c.enabled && !c.separator && !c.header)
    }

    /// Close the menu and tell the callback what was chosen (nothing, for
    /// `None`).
    fn finish(&self, term_window: &mut TermWindow, chosen: Option<usize>) {
        let entry = chosen
            .and_then(|idx| self.args.choices.get(idx))
            .map(|c| InputSelectorEntry {
                label: c.label.clone(),
                id: c.id.clone(),
            });
        let window = GuiWin::new(term_window);
        if let Some((window, _)) = self.popup.borrow_mut().take() {
            window.close_popup();
        }
        term_window.cancel_modal();
        trampoline(self.event_name.clone(), window, self.pane, entry);
    }

    /// The next choosable item from the highlighted one, in `step`'s
    /// direction, wrapping around.
    fn step(&self, step: isize) {
        let count = self.args.choices.len() as isize;
        if count == 0 {
            return;
        }
        let mut idx = match self.selected.get() {
            Some(idx) => idx as isize,
            None if step > 0 => -1,
            None => count,
        };
        for _ in 0..count {
            idx = (idx + step).rem_euclid(count);
            if self.choosable(idx as usize) {
                self.selected.set(Some(idx as usize));
                return;
            }
        }
    }

    fn compute(&self, term_window: &mut TermWindow) -> anyhow::Result<Vec<ComputedElement>> {
        let font = term_window.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let cell_width = metrics.cell_size.width as f32;

        let bg: LinearRgba = term_window.config.command_palette_bg_color.to_linear();
        let fg: LinearRgba = term_window.config.command_palette_fg_color.to_linear();
        // blended as the eye sees it (sRGB), so a step reads the same on
        // a dark menu and a light one; about Windows 11's menu shades
        let mix = |t: f32| {
            let (to_srgb, to_linear) = (|v: f32| v.powf(1. / 2.2), |v: f32| v.powf(2.2));
            let one = |b: f32, f: f32| to_linear(to_srgb(b) + (to_srgb(f) - to_srgb(b)) * t);
            LinearRgba(one(bg.0, fg.0), one(bg.1, fg.1), one(bg.2, fg.2), 1.)
        };
        let (highlight, dim, line) = (mix(0.08), mix(0.45), mix(0.12));
        let with_icons = self.args.choices.iter().any(|c| c.icon.is_some());
        let (pad_left, pad_right, pad_y) = ITEM_PADDING;

        let build = |row_width: Option<f32>| -> Element {
            let mut rows = vec![];
            for (idx, choice) in self.args.choices.iter().enumerate() {
                if choice.separator {
                    rows.push(
                        Element::new(&font, ElementContent::Children(vec![]))
                            .display(DisplayType::Block)
                            .min_height(Some(Dimension::Pixels(1.)))
                            .min_width(row_width.map(Dimension::Pixels))
                            .margin(BoxDimension {
                                left: Dimension::Cells(0.),
                                right: Dimension::Cells(0.),
                                top: Dimension::Cells(0.25),
                                bottom: Dimension::Cells(0.25),
                            })
                            .colors(ElementColors {
                                border: BorderColor::default(),
                                bg: line.into(),
                                text: fg.into(),
                            }),
                    );
                    continue;
                }
                let text = if choice.header || !choice.enabled {
                    dim
                } else {
                    fg
                };
                let row_bg = if self.selected.get() == Some(idx) {
                    highlight
                } else {
                    LinearRgba::TRANSPARENT
                };
                let mut kids = vec![];
                if with_icons {
                    let icon = match &choice.icon {
                        Some(name) => NERD_FONTS.get(name.as_str()).copied().unwrap_or_else(|| {
                            log::error!("nerdfont {name} not found in NERD_FONTS");
                            ' '
                        }),
                        None => ' ',
                    };
                    kids.push(
                        Element::new(&font, ElementContent::Text(icon.to_string()))
                            .min_width(Some(Dimension::Cells(2.))),
                    );
                }
                kids.push(Element::new(
                    &font,
                    ElementContent::Text(choice.label.clone()),
                ));
                let mut row = Element::new(&font, ElementContent::Children(kids))
                    .display(DisplayType::Block)
                    .padding(BoxDimension {
                        left: Dimension::Cells(pad_left),
                        right: Dimension::Cells(pad_right),
                        top: Dimension::Cells(pad_y),
                        bottom: Dimension::Cells(pad_y),
                    })
                    .min_width(
                        row_width
                            .map(|w| Dimension::Pixels(w - (pad_left + pad_right) * cell_width)),
                    )
                    .colors(ElementColors {
                        border: BorderColor::default(),
                        bg: row_bg.into(),
                        text: text.into(),
                    });
                if self.choosable(idx) {
                    row = row.item_type(UIItemType::PopupMenuItem(idx));
                }
                rows.push(row);
            }
            let corner = |poly| SizedPoly {
                width: Dimension::Cells(0.25),
                height: Dimension::Cells(0.25),
                poly,
            };
            Element::new(&font, ElementContent::Children(rows))
                .colors(ElementColors {
                    border: BorderColor::new(line),
                    bg: bg.into(),
                    text: fg.into(),
                })
                .padding(BoxDimension {
                    left: Dimension::Cells(0.),
                    right: Dimension::Cells(0.),
                    top: Dimension::Cells(0.25),
                    bottom: Dimension::Cells(0.25),
                })
                .border(BoxDimension::new(Dimension::Pixels(1.)))
                .border_corners(Some(Corners {
                    top_left: corner(TOP_LEFT_ROUNDED_CORNER),
                    top_right: corner(TOP_RIGHT_ROUNDED_CORNER),
                    bottom_left: corner(BOTTOM_LEFT_ROUNDED_CORNER),
                    bottom_right: corner(BOTTOM_RIGHT_ROUNDED_CORNER),
                }))
        };

        let dimensions = term_window.dimensions;
        let (window_width, window_height) = (
            dimensions.pixel_width as f32,
            dimensions.pixel_height as f32,
        );
        let layout = |term_window: &TermWindow, x: f32, y: f32, element: &Element| {
            term_window.compute_element(
                &LayoutContext {
                    height: DimensionContext {
                        dpi: dimensions.dpi as f32,
                        pixel_max: window_height,
                        pixel_cell: metrics.cell_size.height as f32,
                    },
                    width: DimensionContext {
                        dpi: dimensions.dpi as f32,
                        pixel_max: window_width,
                        pixel_cell: cell_width,
                    },
                    bounds: euclid::rect(x, y, window_width - x, window_height - y),
                    metrics: &metrics,
                    gl_state: term_window.render_state.as_ref().unwrap(),
                    zindex: 100,
                },
                element,
            )
        };

        // first as wide as its widest line, to learn its size; then every
        // line as wide as that (the highlight spans the menu), placed at
        // the pointer and kept inside the window
        let natural = layout(term_window, 0., 0., &build(None))?;
        let row_width = match &natural.content {
            ComputedElementContent::Children(kids) => kids
                .iter()
                .map(|k| k.bounds.width())
                .fold(12. * cell_width, f32::max),
            _ => natural.content_rect.width(),
        };
        let (width, height) = (
            row_width + natural.bounds.width() - natural.content_rect.width(),
            natural.bounds.height(),
        );
        let (x, y) = (self.at.0 + 2., self.at.1 + 2.);
        let x = if x + width > window_width {
            (self.at.0 - width).max(0.)
        } else {
            x
        };
        let y = if y + height > window_height {
            (window_height - height).max(0.)
        } else {
            y
        };
        Ok(vec![layout(term_window, x, y, &build(Some(row_width)))?])
    }
}

impl Modal for PopupMenu {
    fn mouse_event(&self, _event: MouseEvent, _term_window: &mut TermWindow) -> anyhow::Result<()> {
        Ok(())
    }

    fn window_mouse_event(&self, event: &WindowMouseEvent, term_window: &mut TermWindow) -> bool {
        if self.popup.borrow().is_some() {
            // the menu is not in this window: a press here is one
            // outside the menu
            if let WMEK::Press(_) = &event.kind {
                self.finish(term_window, None);
            }
            return true;
        }
        let (x, y) = (event.coords.x as f32, event.coords.y as f32);
        let inside = self
            .bounds
            .get()
            .map_or(false, |b| b.contains(euclid::point2(x, y)));
        let item = term_window
            .ui_items
            .iter()
            .rev()
            .find(|i| i.hit_test(event.coords.x, event.coords.y))
            .and_then(|i| match i.item_type {
                UIItemType::PopupMenuItem(idx) => Some(idx),
                _ => None,
            })
            .filter(|&idx| self.choosable(idx));
        match &event.kind {
            WMEK::Move => {
                if (x - self.at.0).abs() > DRAG_SLOP || (y - self.at.1).abs() > DRAG_SLOP {
                    self.moved.set(true);
                }
                if item.is_some() && item != self.selected.get() {
                    self.selected.set(item);
                    term_window.invalidate_modal();
                }
            }
            WMEK::Press(_) => {
                if inside {
                    self.pressed.set(item);
                } else {
                    self.finish(term_window, None);
                }
            }
            WMEK::Release(_) => {
                // a click on an item, or the opening button dragged to one
                let dragged = self.pressed.get().is_none() && self.moved.get();
                if let Some(idx) = item {
                    if self.pressed.get() == Some(idx) || dragged {
                        self.finish(term_window, Some(idx));
                    }
                }
                self.pressed.set(None);
            }
            WMEK::VertWheel(_) | WMEK::HorzWheel(_) => {}
        }
        true
    }

    fn key_down(
        &self,
        key: KeyCode,
        mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        match (key, mods) {
            (KeyCode::Escape, _) => self.finish(term_window, None),
            (KeyCode::UpArrow, KeyModifiers::NONE) => {
                self.step(-1);
                self.repaint(term_window);
            }
            (KeyCode::DownArrow, KeyModifiers::NONE) => {
                self.step(1);
                self.repaint(term_window);
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if let Some(idx) = self.selected.get().filter(|&idx| self.choosable(idx)) {
                    self.finish(term_window, Some(idx));
                }
            }
            // everything else stays out of the pane while the menu is up
            _ => {}
        }
        Ok(true)
    }

    fn computed_element(
        &self,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<Ref<'_, [ComputedElement]>> {
        if self.popup.borrow().is_some() {
            // nothing of it is drawn in this window
            self.element.borrow_mut().get_or_insert_with(Vec::new);
        }
        if self.element.borrow().is_none() {
            let element = self.compute(term_window)?;
            self.bounds.set(element.first().map(|e| e.bounds));
            self.element.borrow_mut().replace(element);
        }
        Ok(Ref::map(self.element.borrow(), |v| {
            v.as_ref().unwrap().as_slice()
        }))
    }

    fn reconfigure(&self, term_window: &mut TermWindow) {
        self.element.borrow_mut().take();
        // the popup window's picture with the fonts and colours of now
        let window = self.popup.borrow().as_ref().map(|(w, _)| w.clone());
        if let Some(window) = window {
            match self.layout(term_window, &window) {
                Ok(layout) => {
                    window.update_popup(layout.paint(self.selected.get()));
                    self.popup.borrow_mut().replace((window, layout));
                }
                Err(err) => log::error!("PopupMenu: {err:#}"),
            }
        }
    }

    fn popup_event(&self, event: PopupEvent, term_window: &mut TermWindow) {
        let at = |x: isize, y: isize| {
            self.popup
                .borrow()
                .as_ref()
                .and_then(|(_, layout)| layout.choosable_at(x, y))
        };
        match event {
            PopupEvent::Motion(x, y) => {
                self.moved.set(true);
                let item = at(x, y);
                if item != self.selected.get() {
                    self.selected.set(item);
                    self.repaint(term_window);
                }
            }
            PopupEvent::Leave => {
                if self.selected.take().is_some() {
                    self.repaint(term_window);
                }
            }
            PopupEvent::Press(x, y, _) => self.pressed.set(at(x, y)),
            PopupEvent::Release(x, y, _) => {
                // a click on an item, or the opening button dragged to one
                let dragged = self.pressed.get().is_none() && self.moved.get();
                let item = at(x, y);
                let pressed = self.pressed.take();
                if let Some(idx) = item {
                    if pressed == Some(idx) || dragged {
                        self.finish(term_window, Some(idx));
                    }
                }
            }
            PopupEvent::Dismissed => {
                // (its window is gone already)
                self.popup.borrow_mut().take();
                self.finish(term_window, None);
            }
        }
    }

    fn focus_lost(&self, term_window: &mut TermWindow) {
        // a menu in a window of its own goes when the application is
        // left, as any menu does
        if self.popup.borrow().is_some() {
            self.finish(term_window, None);
        }
    }
}
