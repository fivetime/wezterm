//! The tab switcher (`ShowTabSwitcher`): Ctrl+Tab brings up a grid of
//! the window's tabs, each tile with the tab's title and the last lines
//! of its screen, as NativeTerm's grid over Windows Terminal has them.
//! Further Ctrl+Tab / Ctrl+Shift+Tab presses or the arrows move the
//! choice; releasing Ctrl (or Enter) activates the chosen tab, Escape or
//! any other key leaves the tabs as they are. The mouse picks a tile too,
//! and a press outside the grid dismisses it.

use crate::termwindow::box_model::*;
use crate::termwindow::modal::Modal;
use crate::termwindow::render::corners::{
    BOTTOM_LEFT_ROUNDED_CORNER, BOTTOM_RIGHT_ROUNDED_CORNER, TOP_LEFT_ROUNDED_CORNER,
    TOP_RIGHT_ROUNDED_CORNER,
};
use crate::termwindow::render::hover_card::last_lines;
use crate::termwindow::{DimensionContext, TermWindow, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::keyassignment::KeyAssignment;
use config::Dimension;
use mux::Mux;
use std::cell::{Cell, Ref, RefCell};
use wezterm_term::{KeyCode, KeyModifiers, MouseEvent};
use window::color::LinearRgba;
use window::{MouseEvent as WindowMouseEvent, MouseEventKind as WMEK, RectF};

/// A tile's width in DIP, the lines of the screen it shows, the gap
/// between tiles, the grid's own padding and its most columns.
const TILE: f32 = 300.;
const LINES: usize = 8;
const GAP: f32 = 12.;
const PADDING: f32 = 16.;
const COLUMNS: usize = 4;

struct Tile {
    title: String,
    lines: Vec<String>,
    fg: LinearRgba,
    bg: LinearRgba,
}

pub struct TabSwitcher {
    tiles: Vec<Tile>,
    selected: Cell<usize>,
    /// The grid's columns, as last laid out (the arrows' Up and Down).
    columns: Cell<usize>,
    /// Where it was last drawn, for a press outside it.
    bounds: Cell<Option<RectF>>,
    element: RefCell<Option<Vec<ComputedElement>>>,
}

impl TabSwitcher {
    /// The switcher over the window's tabs, the choice `step` tabs from
    /// the active one.
    pub fn new(term_window: &mut TermWindow, step: isize) -> Self {
        let mux = Mux::get();
        let mut tiles = vec![];
        let mut active = 0;
        if let Some(window) = mux.get_window(term_window.mux_window_id) {
            active = window.get_active_tab_idx();
            for tab in window.iter_tabs() {
                // every tab a tile, so a tile's index is its tab's
                let Some(pane) = tab.get_active_pane() else {
                    tiles.push(Tile {
                        title: tab.get_title(),
                        lines: vec![],
                        fg: term_window.config.command_palette_fg_color.to_linear(),
                        bg: term_window.config.command_palette_bg_color.to_linear(),
                    });
                    continue;
                };
                let title = match tab.get_title() {
                    t if t.is_empty() => pane.get_title(),
                    t => t,
                };
                let dims = pane.get_dimensions();
                let top = dims.physical_top;
                let (_, lines) = pane.get_lines(top..top + dims.viewport_rows as isize);
                let lines = last_lines(lines.iter().map(|l| l.as_str().to_string()), LINES);
                let palette = pane.palette();
                tiles.push(Tile {
                    title,
                    lines,
                    fg: palette.foreground.to_linear(),
                    bg: palette.background.to_linear(),
                });
            }
        }
        let count = tiles.len().max(1) as isize;
        Self {
            tiles,
            selected: Cell::new((active as isize + step).rem_euclid(count) as usize),
            columns: Cell::new(1),
            bounds: Cell::new(None),
            element: RefCell::new(None),
        }
    }

    fn select(&self, idx: usize, term_window: &mut TermWindow) {
        if idx != self.selected.get() && idx < self.tiles.len() {
            self.selected.set(idx);
            term_window.invalidate_modal();
        }
    }

    fn step(&self, step: isize, term_window: &mut TermWindow) {
        let count = self.tiles.len() as isize;
        if count > 0 {
            let idx = (self.selected.get() as isize + step).rem_euclid(count);
            self.select(idx as usize, term_window);
        }
    }

    /// Activate the chosen tab and close.
    fn finish(&self, term_window: &mut TermWindow) {
        term_window.cancel_modal();
        if !self.tiles.is_empty() {
            if let Err(err) = term_window.activate_tab(self.selected.get() as isize) {
                log::error!("tab switcher: {err:#}");
            }
        }
    }

    fn compute(&self, term_window: &mut TermWindow) -> anyhow::Result<Vec<ComputedElement>> {
        let title_font = term_window.fonts.title_font()?;
        let term_font = term_window.fonts.default_font()?;
        let metrics = RenderMetrics::with_font_metrics(&title_font.metrics());
        let term_metrics = RenderMetrics::with_font_metrics(&term_font.metrics());
        let scale = term_window.dimensions.dpi as f32 / 96.;
        let px = |v: f32| (v * scale).round();
        let window_w = term_window.dimensions.pixel_width as f32;
        let window_h = term_window.dimensions.pixel_height as f32;

        // as many columns as fit, and tiles narrower when even one does not
        let room = window_w - 2. * px(PADDING) - 2. * px(GAP);
        let columns = (((room + px(GAP)) / (px(TILE) + px(GAP))).floor() as usize)
            .clamp(1, COLUMNS)
            .min(self.tiles.len().max(1));
        let tile_w = px(TILE).min(room).max(px(80.));
        self.columns.set(columns);

        let bg: LinearRgba = term_window.config.command_palette_bg_color.to_linear();
        let fg: LinearRgba = term_window.config.command_palette_fg_color.to_linear();
        let mix = |a: LinearRgba, b: LinearRgba, t: f32| {
            LinearRgba(
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                a.2 + (b.2 - a.2) * t,
                1.,
            )
        };
        let round = |r: f32| {
            let size = Dimension::Pixels(px(r));
            let corner = |poly| SizedPoly {
                width: size,
                height: size,
                poly,
            };
            Corners {
                top_left: corner(TOP_LEFT_ROUNDED_CORNER),
                top_right: corner(TOP_RIGHT_ROUNDED_CORNER),
                bottom_left: corner(BOTTOM_LEFT_ROUNDED_CORNER),
                bottom_right: corner(BOTTOM_RIGHT_ROUNDED_CORNER),
            }
        };
        // rounded corners are filled in the border's colour: each rounded
        // thing is a body in its own colour inside a shell in the ring's
        let shell = |kid: Element, ring: LinearRgba, width: f32, r: f32| {
            Element::new(&title_font, ElementContent::Children(vec![kid]))
                .display(DisplayType::Block)
                .padding(BoxDimension::new(Dimension::Pixels(width)))
                .border_corners(Some(round(r)))
                .colors(ElementColors {
                    border: BorderColor::new(ring),
                    bg: ring.into(),
                    text: fg.into(),
                })
        };
        let plain = |text: LinearRgba| ElementColors {
            border: BorderColor::new(LinearRgba::TRANSPARENT),
            bg: LinearRgba::TRANSPARENT.into(),
            text: text.into(),
        };

        let ring_w = px(2.);
        let inner_w = tile_w - 2. * ring_w;
        let columns_of_text = ((inner_w - 2. * px(10.))
            / term_metrics.cell_size.width.max(1) as f32)
            .max(1.) as usize;
        let mut rows = vec![];
        for (row_idx, row) in self.tiles.chunks(columns).enumerate() {
            let mut kids = vec![];
            for (col, tile) in row.iter().enumerate() {
                let idx = row_idx * columns + col;
                let mut body = vec![Element::new(
                    &title_font,
                    ElementContent::Text(format!("{}: {}", idx + 1, tile.title)),
                )
                .display(DisplayType::Block)
                .colors(plain(tile.fg))];
                // the screen's lines, the tile's height whatever they are
                let mut lines: Vec<Element> = tile
                    .lines
                    .iter()
                    .map(|l| {
                        Element::new(
                            &term_font,
                            ElementContent::Text(l.chars().take(columns_of_text).collect()),
                        )
                        .display(DisplayType::Block)
                        .colors(plain(mix(tile.bg, tile.fg, 0.8)))
                    })
                    .collect();
                while lines.len() < LINES {
                    lines.push(
                        Element::new(&term_font, ElementContent::Text(String::new()))
                            .display(DisplayType::Block),
                    );
                }
                body.push(
                    Element::new(&term_font, ElementContent::Children(lines))
                        .display(DisplayType::Block)
                        .margin(BoxDimension {
                            left: Dimension::Pixels(0.),
                            right: Dimension::Pixels(0.),
                            top: Dimension::Pixels(px(6.)),
                            bottom: Dimension::Pixels(0.),
                        }),
                );
                let body = Element::new(&title_font, ElementContent::Children(body))
                    .display(DisplayType::Block)
                    .min_width(Some(Dimension::Pixels(inner_w)))
                    .max_width(Some(Dimension::Pixels(inner_w)))
                    .padding(BoxDimension::new(Dimension::Pixels(px(10.))))
                    .border_corners(Some(round(6.)))
                    .colors(ElementColors {
                        border: BorderColor::new(tile.bg),
                        bg: tile.bg.into(),
                        text: tile.fg.into(),
                    });
                let ring = if idx == self.selected.get() {
                    fg
                } else {
                    mix(bg, fg, 0.12)
                };
                kids.push(
                    shell(body, ring, ring_w, 8.)
                        .display(DisplayType::Inline)
                        .item_type(UIItemType::TabSwitcherTile(idx))
                        .margin(BoxDimension::new(Dimension::Pixels(px(GAP / 2.)))),
                );
            }
            rows.push(
                Element::new(&title_font, ElementContent::Children(kids))
                    .display(DisplayType::Block),
            );
        }

        let grid_w = columns as f32 * (tile_w + px(GAP)) + 2. * px(PADDING);
        let grid = Element::new(&title_font, ElementContent::Children(rows))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(grid_w - 2.)))
            .max_width(Some(Dimension::Pixels(grid_w - 2.)))
            .padding(BoxDimension::new(Dimension::Pixels(
                px(PADDING) - px(GAP / 2.),
            )))
            .border_corners(Some(round(11.)))
            .colors(ElementColors {
                border: BorderColor::new(bg),
                bg: bg.into(),
                text: fg.into(),
            });
        let grid = shell(grid, mix(bg, fg, 0.15), 1., 12.);

        let x = ((window_w - grid_w) / 2.).max(0.).round();
        let mut computed = term_window.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: term_window.dimensions.dpi as f32,
                    pixel_max: window_h,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: term_window.dimensions.dpi as f32,
                    pixel_max: window_w,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds: euclid::rect(x, 0., grid_w, window_h),
                metrics: &metrics,
                gl_state: term_window.render_state.as_ref().unwrap(),
                zindex: 100,
            },
            &grid,
        )?;
        // in the middle of the window, below the tab bar when it fits
        let top = term_window.tab_bar_pixel_height().unwrap_or(0.);
        let height = computed.bounds.height();
        let y = ((window_h - height) / 2.)
            .max(top.min(window_h - height))
            .max(0.);
        computed.translate(euclid::vec2(0., y.round()));
        self.bounds.set(Some(computed.bounds));
        Ok(vec![computed])
    }

    fn tile_at(&self, term_window: &TermWindow, event: &WindowMouseEvent) -> Option<usize> {
        term_window
            .ui_items
            .iter()
            .rev()
            .find(|i| i.hit_test(event.coords.x, event.coords.y))
            .and_then(|i| match i.item_type {
                UIItemType::TabSwitcherTile(idx) => Some(idx),
                _ => None,
            })
    }
}

fn is_ctrl(key: &KeyCode) -> bool {
    matches!(
        key,
        KeyCode::Control | KeyCode::LeftControl | KeyCode::RightControl
    )
}

impl Modal for TabSwitcher {
    fn perform_assignment(
        &self,
        _assignment: &KeyAssignment,
        _term_window: &mut TermWindow,
    ) -> bool {
        false
    }

    fn mouse_event(&self, _event: MouseEvent, _term_window: &mut TermWindow) -> anyhow::Result<()> {
        Ok(())
    }

    fn window_mouse_event(&self, event: &WindowMouseEvent, term_window: &mut TermWindow) -> bool {
        let tile = self.tile_at(term_window, event);
        match &event.kind {
            WMEK::Move => {
                if let Some(idx) = tile {
                    self.select(idx, term_window);
                }
            }
            WMEK::Press(_) => match tile {
                Some(idx) => {
                    self.selected.set(idx);
                    self.finish(term_window);
                }
                None => {
                    let (x, y) = (event.coords.x as f32, event.coords.y as f32);
                    let inside = self
                        .bounds
                        .get()
                        .map_or(false, |b| b.contains(euclid::point2(x, y)));
                    if !inside {
                        term_window.cancel_modal();
                    }
                }
            },
            _ => {}
        }
        true
    }

    fn key_down(
        &self,
        key: KeyCode,
        mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        let columns = self.columns.get().max(1) as isize;
        match key {
            _ if is_ctrl(&key) => {}
            // Tab comes as a character with Ctrl held
            KeyCode::Tab | KeyCode::Char('\t') if mods.contains(KeyModifiers::SHIFT) => {
                self.step(-1, term_window)
            }
            KeyCode::Tab | KeyCode::Char('\t') => self.step(1, term_window),
            KeyCode::RightArrow => self.step(1, term_window),
            KeyCode::LeftArrow => self.step(-1, term_window),
            KeyCode::DownArrow => self.step(columns, term_window),
            KeyCode::UpArrow => self.step(-columns, term_window),
            KeyCode::Enter => self.finish(term_window),
            KeyCode::Escape => term_window.cancel_modal(),
            // the tabs as they were; the key goes on
            _ => {
                term_window.cancel_modal();
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn key_up(
        &self,
        key: KeyCode,
        _mods: KeyModifiers,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<bool> {
        if is_ctrl(&key) {
            self.finish(term_window);
            return Ok(true);
        }
        Ok(false)
    }

    fn computed_element(
        &self,
        term_window: &mut TermWindow,
    ) -> anyhow::Result<Ref<'_, [ComputedElement]>> {
        if self.element.borrow().is_none() {
            let element = self.compute(term_window)?;
            self.element.borrow_mut().replace(element);
        }
        Ok(Ref::map(self.element.borrow(), |v| {
            v.as_ref().unwrap().as_slice()
        }))
    }

    fn reconfigure(&self, _term_window: &mut TermWindow) {
        self.element.borrow_mut().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ctrl_releases_it() {
        assert!(is_ctrl(&KeyCode::Control));
        assert!(is_ctrl(&KeyCode::LeftControl));
        assert!(is_ctrl(&KeyCode::RightControl));
        assert!(!is_ctrl(&KeyCode::Tab));
        assert!(!is_ctrl(&KeyCode::Shift));
    }
}
