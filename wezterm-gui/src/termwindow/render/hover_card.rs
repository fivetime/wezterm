//! A tab's hover card (`show_tab_hover_cards`), as Chrome shows one: the
//! pointer resting on a tab for a moment brings up a card under it with
//! the tab's name, a line about it and the last lines of its screen.
//!
//! What the card says can come from the configuration: the
//! `tab-hover-card` event gets the tab's and its active pane's ids and
//! returns `{ title = ..., note = ... }` (nil for the tab's own title,
//! false for no card). It is asked when the pointer comes to the tab, as
//! an ordinary (asynchronous) event, so the handler may run programs; the
//! card waits for its answer, `WAIT` at most.

use crate::tabbar::TabBarItem;
use crate::termwindow::box_model::*;
use crate::termwindow::render::corners::*;
use crate::termwindow::{TermWindowNotif, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{Dimension, DimensionContext};
use mlua::FromLua;
use mux::tab::TabId;
use mux::Mux;
use std::rc::Rc;
use std::time::{Duration, Instant};
use window::color::LinearRgba;
use window::WindowOps;

/// How long the pointer rests on a tab before its card shows.
const DELAY: Duration = Duration::from_millis(600);
/// How long after that the card waits for the configuration's answer
/// before it shows the tab's own title.
const WAIT: Duration = Duration::from_millis(1000);
/// The card's width in DIP, and how many of the screen's lines it shows.
const WIDTH: f32 = 380.;
const LINES: usize = 6;

/// The configuration's answer: `None` for the tab's own card (no handler,
/// or nil), `Some(None)` for no card, else the title and the note.
type Answer = Option<Option<(String, String)>>;

pub struct TabHoverCard {
    tab_idx: usize,
    tab_id: TabId,
    since: Instant,
    /// Once the configuration answered.
    answer: Option<Answer>,
    /// Once built: what it shows, or `None` for no card.
    content: Option<Option<Content>>,
}

#[derive(Clone)]
struct Content {
    title: String,
    note: String,
    lines: Vec<String>,
}

async fn ask_lua(
    lua: Option<Rc<mlua::Lua>>,
    tab_id: TabId,
    pane_id: mux::pane::PaneId,
) -> anyhow::Result<Answer> {
    let Some(lua) = lua else { return Ok(None) };
    let v =
        config::lua::emit_async_callback(&*lua, ("tab-hover-card".to_string(), (tab_id, pane_id)))
            .await?;
    Ok(match v {
        mlua::Value::Boolean(false) => Some(None),
        mlua::Value::Table(t) => {
            let title: Option<String> = t.get("title")?;
            let note: Option<String> = t.get("note")?;
            Some(Some((title.unwrap_or_default(), note.unwrap_or_default())))
        }
        mlua::Value::String(_) => Some(Some((String::from_lua(v, &*lua)?, String::new()))),
        _ => None,
    })
}

/// The last non-empty lines of a screen, `count` at most, in order.
pub(crate) fn last_lines(lines: impl Iterator<Item = String>, count: usize) -> Vec<String> {
    let mut all: Vec<String> = lines.map(|l| l.trim_end().to_string()).collect();
    while all.last().is_some_and(|l| l.is_empty()) {
        all.pop();
    }
    let skip = all.len().saturating_sub(count);
    all.into_iter().skip(skip).collect()
}

/// A bubble's shadow under `element` (Chrome's bubbles stand out from
/// the page by it, their colour being the window's): rings of shadow
/// around it, deeper below, each a shell in its translucent colour whose
/// corners (filled in that colour) round it. Returns the element and how
/// far the shadow reaches to the left and above.
pub(crate) fn shadowed(
    font: &std::rc::Rc<wezterm_font::LoadedFont>,
    element: Element,
    radius: f32,
    scale: f32,
    dark: bool,
) -> (Element, (f32, f32)) {
    const RINGS: usize = 4;
    let ring = (1.5 * scale).round().max(1.);
    let shade = LinearRgba(0., 0., 0., if dark { 0.12 } else { 0.035 });
    let mut element = element;
    for i in 1..=RINGS {
        let r = Dimension::Pixels(radius + ring * i as f32);
        let corner = |poly| SizedPoly {
            width: r,
            height: r,
            poly,
        };
        element = Element::new(font, ElementContent::Children(vec![element]))
            .display(DisplayType::Block)
            .padding(BoxDimension {
                left: Dimension::Pixels(ring),
                right: Dimension::Pixels(ring),
                top: Dimension::Pixels((ring / 2.).round()),
                bottom: Dimension::Pixels(ring + (ring / 2.).round()),
            })
            .border_corners(Some(Corners {
                top_left: corner(TOP_LEFT_ROUNDED_CORNER),
                top_right: corner(TOP_RIGHT_ROUNDED_CORNER),
                bottom_left: corner(BOTTOM_LEFT_ROUNDED_CORNER),
                bottom_right: corner(BOTTOM_RIGHT_ROUNDED_CORNER),
            }))
            .colors(ElementColors {
                border: BorderColor::new(shade),
                bg: shade.into(),
                text: InheritableColor::Inherited,
            });
    }
    let reach = RINGS as f32;
    (element, (ring * reach, (ring / 2.).round() * reach))
}

impl crate::TermWindow {
    /// The pointer moved: over the tab `tab` (its body or close button),
    /// or not over a tab. A card waits `DELAY` on a new tab; a press or
    /// leaving takes it away.
    pub fn hover_tab(&mut self, tab: Option<usize>, pressed: bool) {
        if !self.config.show_tab_hover_cards || pressed {
            self.tab_hover_card = None;
            return;
        }
        let Some(idx) = tab else {
            self.tab_hover_card = None;
            return;
        };
        if self
            .tab_hover_card
            .as_ref()
            .is_some_and(|c| c.tab_idx == idx)
        {
            return;
        }
        self.tab_hover_card = None;
        let Some((tab_id, pane_id)) = Mux::get()
            .get_window(self.mux_window_id)
            .and_then(|w| w.get_tab_at_idx(idx).cloned())
            .and_then(|t| Some((t.tab_id(), t.get_active_pane()?.pane_id())))
        else {
            return;
        };
        let since = Instant::now();
        self.tab_hover_card = Some(TabHoverCard {
            tab_idx: idx,
            tab_id,
            since,
            answer: None,
            content: None,
        });
        self.update_next_frame_time(Some(since + DELAY));

        // ask now: the answer is there by the time the card shows
        let Some(window) = self.window.clone() else {
            return;
        };
        promise::spawn::spawn(config::with_lua_config_on_main_thread(
            move |lua| async move {
                let answer = ask_lua(lua, tab_id, pane_id).await.unwrap_or_else(|err| {
                    log::warn!("tab-hover-card: {err:#}");
                    None
                });
                window.notify(TermWindowNotif::Apply(Box::new(move |tw| {
                    if let Some(card) = tw.tab_hover_card.as_mut() {
                        if card.tab_id == tab_id && card.answer.is_none() {
                            card.answer = Some(answer);
                            if let Some(w) = tw.window.as_ref() {
                                w.invalidate();
                            }
                        }
                    }
                })));
                Ok(())
            },
        ))
        .detach();
    }

    fn hover_card_content(&self, tab_idx: usize, answer: Answer) -> Option<Content> {
        let mux = Mux::get();
        let (tab, pane) = {
            let window = mux.get_window(self.mux_window_id)?;
            let tab = window.get_tab_at_idx(tab_idx)?.clone();
            let pane = tab.get_active_pane()?;
            (tab, pane)
        };
        let own_title = {
            let t = tab.get_title();
            if t.is_empty() {
                pane.get_title()
            } else {
                t
            }
        };
        let (title, note) = match answer {
            Some(None) => return None,
            Some(Some((title, note))) if !title.is_empty() => {
                // the name the configuration gives, the tab's own title
                // beside what it says when they differ
                let note = if title != own_title && !own_title.is_empty() {
                    vec![own_title, note]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                } else {
                    note
                };
                (title, note)
            }
            Some(Some((_, note))) => (own_title, note),
            None => (own_title, String::new()),
        };
        let dims = pane.get_dimensions();
        let top = dims.physical_top;
        let (_, lines) = pane.get_lines(top..top + dims.viewport_rows as isize);
        let lines = last_lines(lines.iter().map(|l| l.as_str().to_string()), LINES);
        Some(Content { title, note, lines })
    }

    /// The card, over everything else, once the pointer has rested long
    /// enough.
    pub fn paint_hover_card(&mut self) -> anyhow::Result<()> {
        if !self.config.show_tab_hover_cards || self.get_modal().is_some() {
            return Ok(());
        }
        let Some((tab_idx, since, answer, built)) = self
            .tab_hover_card
            .as_ref()
            .map(|c| (c.tab_idx, c.since, c.answer.clone(), c.content.is_some()))
        else {
            return Ok(());
        };
        if since.elapsed() < DELAY {
            self.update_next_frame_time(Some(since + DELAY));
            return Ok(());
        }
        if !built {
            let answer = match answer {
                Some(answer) => answer,
                None if since.elapsed() < DELAY + WAIT => {
                    // the answer invalidates the window when it comes
                    self.update_next_frame_time(Some(since + DELAY + WAIT));
                    return Ok(());
                }
                None => None,
            };
            let content = self.hover_card_content(tab_idx, answer);
            if let Some(card) = self.tab_hover_card.as_mut() {
                card.content = Some(content);
            }
        }
        let Some(tab) = self.ui_items.iter().find(|i| {
            matches!(i.item_type, UIItemType::TabBar(TabBarItem::Tab { tab_idx: t, .. }) if t == tab_idx)
        }) else {
            return Ok(());
        };
        let (tab_x, tab_bottom) = (tab.x as f32, (tab.y + tab.height) as f32);
        let Some(Some(content)) = self.tab_hover_card.as_ref().and_then(|c| c.content.clone())
        else {
            return Ok(());
        };

        let scale = self.dimensions.dpi as f32 / 96.;
        let dip = |v: f32| Dimension::Pixels((v * scale).round());
        let title_font = self.fonts.title_font()?;
        let term_font = self.fonts.default_font()?;
        let metrics = RenderMetrics::with_font_metrics(&title_font.metrics());
        let term_metrics = RenderMetrics::with_font_metrics(&term_font.metrics());
        let window_w = self.dimensions.pixel_width as f32;
        let width = (WIDTH * scale).min(window_w - 16. * scale).max(0.);

        let bg: LinearRgba = self.config.command_palette_bg_color.to_linear();
        let fg: LinearRgba = self.config.command_palette_fg_color.to_linear();
        let palette = self.palette().clone();
        let screen_bg = palette.background.to_linear();
        let screen_fg = palette.foreground.to_linear();
        let plain = |text| ElementColors {
            border: BorderColor::new(LinearRgba::TRANSPARENT),
            bg: LinearRgba::TRANSPARENT.into(),
            text,
        };
        let round = |r: f32| {
            let size = dip(r);
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

        let mut kids = vec![
            Element::new(&title_font, ElementContent::Text(content.title.clone()))
                .display(DisplayType::Block)
                .colors(plain(fg.into())),
        ];
        if !content.note.is_empty() {
            kids.push(
                Element::new(&title_font, ElementContent::Text(content.note.clone()))
                    .display(DisplayType::Block)
                    .colors(plain(fg.mul_alpha(0.65).into()))
                    .margin(BoxDimension {
                        left: dip(0.),
                        right: dip(0.),
                        top: dip(2.),
                        bottom: dip(0.),
                    }),
            );
        }
        if !content.lines.is_empty() {
            // as many characters as the card holds
            let inner = width - 2. * (12. + 8.) * scale;
            let columns = (inner / term_metrics.cell_size.width.max(1) as f32).max(1.) as usize;
            let lines = content
                .lines
                .iter()
                .map(|l| {
                    Element::new(
                        &term_font,
                        ElementContent::Text(l.chars().take(columns).collect()),
                    )
                    .display(DisplayType::Block)
                    .colors(plain(screen_fg.into()))
                })
                .collect();
            kids.push(
                Element::new(&term_font, ElementContent::Children(lines))
                    .display(DisplayType::Block)
                    .padding(BoxDimension::new(dip(8.)))
                    .margin(BoxDimension {
                        left: dip(0.),
                        right: dip(0.),
                        top: dip(10.),
                        bottom: dip(0.),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(round(6.)))
                    .colors(ElementColors {
                        border: BorderColor::new(screen_bg),
                        bg: screen_bg.into(),
                        text: screen_fg.into(),
                    }),
            );
        }
        // rounded corners are filled in the border's colour: the body in
        // its own colour inside a 1-pixel shell in the outline's
        let body = Element::new(&title_font, ElementContent::Children(kids))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(width - 2.)))
            .max_width(Some(Dimension::Pixels(width - 2.)))
            .padding(BoxDimension::new(dip(12.)))
            .border_corners(Some(round(7.)))
            .colors(ElementColors {
                border: BorderColor::new(bg),
                bg: bg.into(),
                text: fg.into(),
            });
        let outline = LinearRgba(
            bg.0 + (fg.0 - bg.0) * 0.15,
            bg.1 + (fg.1 - bg.1) * 0.15,
            bg.2 + (fg.2 - bg.2) * 0.15,
            1.,
        );
        let card = Element::new(&title_font, ElementContent::Children(vec![body]))
            .display(DisplayType::Block)
            .min_width(Some(Dimension::Pixels(width)))
            .max_width(Some(Dimension::Pixels(width)))
            .padding(BoxDimension::new(Dimension::Pixels(1.)))
            .border_corners(Some(round(8.)))
            .colors(ElementColors {
                border: BorderColor::new(outline),
                bg: outline.into(),
                text: fg.into(),
            });

        let (card, (left, top)) = shadowed(
            &title_font,
            card,
            (8. * scale).round(),
            scale,
            (bg.0 + bg.1 + bg.2) / 3. < 0.2,
        );
        let x = tab_x.min(window_w - width - 8. * scale).max(8. * scale) - left;
        let y = tab_bottom + 4. * scale - top;
        let gl_state = self.render_state.as_ref().unwrap();
        let computed = self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_height as f32,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: window_w,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds: euclid::rect(
                    x,
                    y,
                    width + 2. * left,
                    (self.dimensions.pixel_height as f32 - y).max(0.),
                ),
                metrics: &metrics,
                gl_state,
                zindex: 100,
            },
            &card,
        )?;
        self.render_element(&computed, gl_state, None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screens_last_lines() {
        let lines = vec!["a", "b  ", "", "c", "d", "e", "f", "g", "", "  "];
        assert_eq!(
            last_lines(lines.into_iter().map(String::from), 6),
            ["", "c", "d", "e", "f", "g"]
        );
        assert_eq!(
            last_lines(vec!["", ""].into_iter().map(String::from), 6),
            Vec::<String>::new()
        );
    }
}
