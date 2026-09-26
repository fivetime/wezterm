use crate::customglyph::*;
use crate::tabbar::{TabBarItem, TabEntry};
use crate::termwindow::box_model::*;
use crate::termwindow::render::corners::*;

use crate::termwindow::render::chrome_tabs;
use crate::termwindow::render::window_buttons::window_button_element;
use crate::termwindow::{UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::{Dimension, DimensionContext, TabBarColors};
use std::rc::Rc;
use wezterm_font::LoadedFont;
use wezterm_term::color::{ColorAttribute, ColorPalette};
use window::color::LinearRgba;
use window::RectF;
use window::{IntegratedTitleButton, IntegratedTitleButtonStyle};

const X_BUTTON: &[Poly] = &[
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::One, BlockCoord::Zero),
            PolyCommand::LineTo(BlockCoord::Zero, BlockCoord::One),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::Zero, BlockCoord::Zero),
            PolyCommand::LineTo(BlockCoord::One, BlockCoord::One),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
];

/// The tab search button's icon: two overlapping windows (the button
/// opens the tab switcher's grid, not a menu, so not Chrome's chevron):
/// a square in front, the back one's top and right edges behind it,
/// across a 13-unit box.
const TAB_SWITCHER: &[Poly] = &[
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::Zero, BlockCoord::Frac(4, 13)),
            PolyCommand::LineTo(BlockCoord::Frac(9, 13), BlockCoord::Frac(4, 13)),
            PolyCommand::LineTo(BlockCoord::Frac(9, 13), BlockCoord::One),
            PolyCommand::LineTo(BlockCoord::Zero, BlockCoord::One),
            PolyCommand::LineTo(BlockCoord::Zero, BlockCoord::Frac(4, 13)),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::Frac(4, 13), BlockCoord::Frac(4, 13)),
            PolyCommand::LineTo(BlockCoord::Frac(4, 13), BlockCoord::Zero),
            PolyCommand::LineTo(BlockCoord::One, BlockCoord::Zero),
            PolyCommand::LineTo(BlockCoord::One, BlockCoord::Frac(9, 13)),
            PolyCommand::LineTo(BlockCoord::Frac(9, 13), BlockCoord::Frac(9, 13)),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
];

const PLUS_BUTTON: &[Poly] = &[
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::Frac(1, 2), BlockCoord::Zero),
            PolyCommand::LineTo(BlockCoord::Frac(1, 2), BlockCoord::One),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
    Poly {
        path: &[
            PolyCommand::MoveTo(BlockCoord::Zero, BlockCoord::Frac(1, 2)),
            PolyCommand::LineTo(BlockCoord::One, BlockCoord::Frac(1, 2)),
        ],
        intensity: BlockAlpha::Full,
        style: PolyStyle::Outline,
    },
];

impl crate::TermWindow {
    pub fn invalidate_fancy_tab_bar(&mut self) {
        self.fancy_tab_bar.take();
        // (the hover card's layout holds glyphs of the same atlas)
        if let Some(card) = self.tab_hover_card.as_mut() {
            card.computed = None;
        }
    }

    pub fn build_fancy_tab_bar(&self, palette: &ColorPalette) -> anyhow::Result<ComputedElement> {
        let _timer = crate::stats::Timed::new("fancy_tab_bar.build");
        let tab_bar_height = self.tab_bar_pixel_height()?;
        let font = self.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let items = self.tab_bar.items();
        let colors = self
            .config
            .colors
            .as_ref()
            .and_then(|c| c.tab_bar.as_ref())
            .cloned()
            .unwrap_or_else(TabBarColors::default);

        let mut left_status = vec![];
        let mut left_eles = vec![];
        let mut right_eles = vec![];
        // Chrome's strip: its sizes in DIP, the strip and the tabs'
        // fills painted beneath the elements (paint_chrome_tabs)
        let chrome = self.config.tab_strip_style == config::TabStripStyle::Chrome;
        let scale = self.dimensions.dpi as f32 / 96.;
        let dip = |v: f32| Dimension::Pixels((v * scale).round());
        let dip_px = |v: f32| (v * scale).round();
        let frame_colour: config::SrgbaTuple = if self.focused.is_some() {
            self.config.window_frame.active_titlebar_bg
        } else {
            self.config.window_frame.inactive_titlebar_bg
        }
        .into();
        let mut bar_colors = ElementColors {
            border: BorderColor::default(),
            bg: if self.focused.is_some() {
                self.config.window_frame.active_titlebar_bg
            } else {
                self.config.window_frame.inactive_titlebar_bg
            }
            .to_linear()
            .into(),
            text: if self.focused.is_some() {
                self.config.window_frame.active_titlebar_fg
            } else {
                self.config.window_frame.inactive_titlebar_fg
            }
            .to_linear()
            .into(),
        };
        if chrome {
            bar_colors.bg = window::color::LinearRgba::TRANSPARENT.into();
        }
        // Chrome's strip in pixels (the chrome-strip crate): every size
        // and position below comes from it
        let layout = self.chrome_layout(&font, &metrics)?;
        // the tabs among the items, in order: each takes its own layout
        let mut tab_no = 0usize;
        let tab_vertical_alignment = if self.config.tab_bar_at_bottom || chrome {
            VerticalAlign::Top
        } else {
            VerticalAlign::Bottom
        };

        let item_to_elem = |item: &TabEntry| -> Element {
            let element = Element::with_line(&font, &item.title, palette);

            let bg_color = item
                .title
                .get_cell(0)
                .and_then(|c| match c.attrs().background() {
                    ColorAttribute::Default => None,
                    col => Some(palette.resolve_bg(col)),
                });
            let fg_color = item
                .title
                .get_cell(0)
                .and_then(|c| match c.attrs().foreground() {
                    ColorAttribute::Default => None,
                    col => Some(palette.resolve_fg(col)),
                });

            let new_tab = colors.new_tab();
            let new_tab_hover = colors.new_tab_hover();
            let active_tab = colors.active_tab();
            let is_bottom = self.config.tab_bar_at_bottom;

            match item.item {
                TabBarItem::RightStatus | TabBarItem::LeftStatus | TabBarItem::None => element
                    .item_type(UIItemType::TabBar(TabBarItem::None))
                    .line_height(Some(1.75))
                    .margin(BoxDimension {
                        left: Dimension::Cells(0.),
                        right: Dimension::Cells(0.),
                        top: Dimension::Cells(0.0),
                        bottom: Dimension::Cells(0.),
                    })
                    .padding(BoxDimension {
                        left: Dimension::Cells(0.5),
                        right: Dimension::Cells(0.),
                        top: Dimension::Cells(0.),
                        bottom: Dimension::Cells(0.),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(0.)))
                    .colors(bar_colors.clone()),
                TabBarItem::TabSearchButton => {
                    // Chrome's tab search button: 28 round, 6 into the
                    // region, the glyph in a 13-DIP box centred in the
                    // 16 icon; under the pointer the colour contrasting
                    // most with the strip at 0.16, as the new-tab button
                    let fg = new_tab.fg_color.to_linear();
                    let (button, icon) = match (layout.tab_search, layout.tab_search_icon) {
                        (Some(b), Some(c)) => (b, c),
                        _ => {
                            return Element::new(&font, ElementContent::Text(String::new()))
                                .item_type(UIItemType::TabBar(TabBarItem::None));
                        }
                    };
                    let glyph_inset = dip_px(1.5).round();
                    let chevron = chrome_strip::Rect::new(
                        icon.x + glyph_inset,
                        icon.y + glyph_inset,
                        icon.w - 2. * glyph_inset,
                        icon.h - 2. * glyph_inset,
                    );
                    let hover = chrome_tabs::max_contrast(frame_colour)
                        .to_linear()
                        .mul_alpha(chrome_tabs::HIGHLIGHT);
                    // before the tabs: past the caption buttons at the
                    // left; trailing: past the new-tab button
                    let before: f32 = if layout.tab_search_trailing {
                        layout.new_tab.right()
                    } else {
                        layout
                            .left_placed
                            .iter()
                            .map(|b| b.width + b.margin_left + b.margin_right)
                            .sum()
                    };
                    let pad_x = ((button.w - chevron.w) / 2. - 1.).max(0.).floor();
                    let pad_top = (chevron.y - button.y - 1.).max(0.).round();
                    let pad_bottom = (button.h - 2. - chevron.h - pad_top).max(0.);
                    Element::new(
                        &font,
                        ElementContent::Poly {
                            line_width: metrics.underline_height.max(2),
                            poly: SizedPoly {
                                poly: TAB_SWITCHER,
                                width: Dimension::Pixels(chevron.w),
                                height: Dimension::Pixels(chevron.h),
                            },
                        },
                    )
                    .vertical_align(VerticalAlign::Top)
                    .item_type(UIItemType::TabBar(item.item.clone()))
                    .margin(BoxDimension {
                        left: Dimension::Pixels((button.x - before).max(0.)),
                        right: dip(0.),
                        top: Dimension::Pixels(button.y),
                        bottom: Dimension::Pixels(layout.height - button.y - button.h),
                    })
                    .padding(BoxDimension {
                        left: Dimension::Pixels(pad_x),
                        right: Dimension::Pixels(pad_x),
                        top: Dimension::Pixels(pad_top),
                        bottom: Dimension::Pixels(pad_bottom),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(chrome_circle(button.w / 2.)))
                    .colors(ElementColors {
                        border: BorderColor::new(window::color::LinearRgba::TRANSPARENT),
                        bg: window::color::LinearRgba::TRANSPARENT.into(),
                        text: fg.into(),
                    })
                    .hover_colors(Some(ElementColors {
                        border: BorderColor::new(hover),
                        bg: hover.into(),
                        text: fg.into(),
                    }))
                }
                TabBarItem::NewTabButton if chrome => {
                    // Chrome's new-tab button, built as the tab search
                    // button is: 28 round, 6 down the strip (its border
                    // insets of 6 above and 7 below centre it in the 40
                    // above the toolbar's line), the plus 10 across in
                    // the 16 icon (kAddOldIcon: 3 to 13); under the
                    // pointer the colour contrasting most with the strip
                    // at 0.16 (kColorTabStripControlButtonInkDrop:
                    // GetColorWithMaxContrast of the inactive tab's
                    // background, the strip's with a system theme)
                    let fg = new_tab.fg_color.to_linear();
                    let (button, icon) = (layout.new_tab, layout.new_tab_icon);
                    let glyph_inset = dip_px(3.);
                    let plus = chrome_strip::Rect::new(
                        icon.x + glyph_inset,
                        icon.y + glyph_inset,
                        icon.w - 2. * glyph_inset,
                        icon.h - 2. * glyph_inset,
                    );
                    let hover = chrome_tabs::max_contrast(frame_colour)
                        .to_linear()
                        .mul_alpha(chrome_tabs::HIGHLIGHT);
                    let before = layout
                        .tabs
                        .iter()
                        .rfind(|t| t.visible)
                        .map_or(0., |t| t.body.right());
                    let pad_x = ((button.w - plus.w) / 2. - 1.).max(0.).floor();
                    let pad_top = (plus.y - button.y - 1.).max(0.).round();
                    let pad_bottom = (button.h - 2. - plus.h - pad_top).max(0.);
                    Element::new(
                        &font,
                        ElementContent::Poly {
                            line_width: metrics.underline_height.max(2),
                            poly: SizedPoly {
                                poly: PLUS_BUTTON,
                                width: Dimension::Pixels(plus.w),
                                height: Dimension::Pixels(plus.h),
                            },
                        },
                    )
                    .vertical_align(VerticalAlign::Top)
                    .item_type(UIItemType::TabBar(item.item.clone()))
                    .margin(BoxDimension {
                        left: Dimension::Pixels((button.x - before).max(0.)),
                        right: dip(0.),
                        top: Dimension::Pixels(button.y),
                        bottom: Dimension::Pixels(layout.height - button.y - button.h),
                    })
                    .padding(BoxDimension {
                        left: Dimension::Pixels(pad_x),
                        right: Dimension::Pixels(pad_x),
                        top: Dimension::Pixels(pad_top),
                        bottom: Dimension::Pixels(pad_bottom),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(chrome_circle(button.w / 2.)))
                    .colors(ElementColors {
                        border: BorderColor::new(window::color::LinearRgba::TRANSPARENT),
                        bg: window::color::LinearRgba::TRANSPARENT.into(),
                        text: fg.into(),
                    })
                    .hover_colors(Some(ElementColors {
                        border: BorderColor::new(hover),
                        bg: hover.into(),
                        text: fg.into(),
                    }))
                }
                TabBarItem::NewTabButton => Element::new(
                    &font,
                    ElementContent::Poly {
                        line_width: metrics.underline_height.max(2),
                        poly: SizedPoly {
                            poly: PLUS_BUTTON,
                            width: Dimension::Pixels(metrics.cell_size.height as f32 / 2.),
                            height: Dimension::Pixels(metrics.cell_size.height as f32 / 2.),
                        },
                    },
                )
                .vertical_align(VerticalAlign::Middle)
                .item_type(UIItemType::TabBar(item.item.clone()))
                .margin(BoxDimension {
                    left: Dimension::Cells(0.5),
                    right: Dimension::Cells(0.),
                    top: Dimension::Cells(0.2),
                    bottom: Dimension::Cells(0.),
                })
                .padding(BoxDimension {
                    left: Dimension::Cells(0.5),
                    right: Dimension::Cells(0.5),
                    top: Dimension::Cells(0.2),
                    bottom: Dimension::Cells(0.25),
                })
                .border(BoxDimension::new(Dimension::Pixels(1.)))
                .colors(ElementColors {
                    border: BorderColor::default(),
                    bg: new_tab.bg_color.to_linear().into(),
                    text: new_tab.fg_color.to_linear().into(),
                })
                .hover_colors(Some(ElementColors {
                    border: BorderColor::default(),
                    bg: new_tab_hover.bg_color.to_linear().into(),
                    text: new_tab_hover.fg_color.to_linear().into(),
                })),
                TabBarItem::Tab { active, tab_idx } if chrome => {
                    let t = &layout.tabs[tab_idx.min(layout.tabs.len().saturating_sub(1))];
                    let tab = if active {
                        active_tab.clone()
                    } else {
                        colors.inactive_tab()
                    };
                    let clear = window::color::LinearRgba::TRANSPARENT;
                    // the title's colour reaches Chrome's contrast against
                    // its tab (the active tab's own colour, else the
                    // strip's), whatever the theme gave
                    let focused = self.focused.is_some();
                    let tab_bg: config::SrgbaTuple = if active {
                        tab.bg_color.into()
                    } else {
                        frame_colour
                    };
                    let readable = |c: config::SrgbaTuple| {
                        chrome_strip::title_colour(c, tab_bg, active, focused).to_linear()
                    };
                    let fg = readable(fg_color.unwrap_or_else(|| tab.fg_color.into()));
                    let hover_fg = match colors.inactive_tab_hover.as_ref() {
                        Some(hover) if !active && fg_color.is_none() => {
                            readable(hover.fg_color.into())
                        }
                        _ => fg,
                    };
                    // the tab's contents (favicon, title, close button)
                    // centred on the highlight's 28 DIP, 6 down the strip,
                    // as Chrome lays a tab out
                    element
                        .vertical_align(VerticalAlign::Top)
                        .item_type(UIItemType::TabBar(item.item.clone()))
                        // each tab exactly where the layout puts it: the
                        // gap to the previous tab shown as its left margin,
                        // none on the right. Two margins of GAP/2 rounded
                        // each to whole pixels made every tab a fraction of
                        // a pixel wider than the layout's, and with many
                        // tabs the new-tab button, placed after the last
                        // one, went past the strip into the caption buttons
                        .margin(BoxDimension {
                            left: match layout.tabs[..tab_idx.min(layout.tabs.len())]
                                .iter()
                                .rfind(|p| p.visible)
                            {
                                Some(previous) => {
                                    Dimension::Pixels(t.body.x - previous.body.right())
                                }
                                None => dip(chrome_tabs::GAP / 2.),
                            },
                            right: dip(0.),
                            top: Dimension::Pixels(layout.tab_top),
                            bottom: Dimension::Pixels(
                                layout.height - layout.tab_top - layout.tab_height,
                            ),
                        })
                        .padding(BoxDimension {
                            // to the first thing shown: the favicon (at
                            // the contents' edge, or centred in a narrow
                            // tab), else the close button where it stands
                            // (a narrow active tab's escapes the contents,
                            // Center(width, 16)), else the contents' edge
                            left: Dimension::Pixels(
                                t.favicon
                                    .map(|f| f.x - t.body.x)
                                    .or_else(|| {
                                        t.close.map(|c| {
                                            (c.x - t.body.x).min(dip_px(chrome_tabs::INSET))
                                        })
                                    })
                                    .unwrap_or(dip_px(chrome_tabs::INSET)),
                            ),
                            // from the last: the close button (2 DIP short
                            // of the body), a centred favicon, else the edge
                            right: Dimension::Pixels(
                                t.close
                                    .map(|c| t.body.right() - c.right())
                                    .or_else(|| {
                                        t.favicon
                                            .filter(|_| t.favicon_centred)
                                            .map(|f| t.body.right() - f.right())
                                    })
                                    .unwrap_or(dip_px(chrome_tabs::INSET)),
                            ),
                            top: dip(0.),
                            bottom: dip(0.),
                        })
                        .min_width(Some(Dimension::Pixels(t.body.w)))
                        .min_height(Some(Dimension::Pixels(layout.tab_height)))
                        .border(BoxDimension::new(Dimension::Pixels(0.)))
                        .colors(ElementColors {
                            border: BorderColor::new(clear),
                            bg: clear.into(),
                            text: fg.into(),
                        })
                        .hover_colors(Some(ElementColors {
                            border: BorderColor::new(clear),
                            bg: clear.into(),
                            text: hover_fg.into(),
                        }))
                }
                TabBarItem::Tab { active, .. } if active => element
                    .vertical_align(tab_vertical_alignment)
                    .item_type(UIItemType::TabBar(item.item.clone()))
                    .margin(if is_bottom {
                        BoxDimension {
                            left: Dimension::Cells(0.),
                            right: Dimension::Cells(0.),
                            top: Dimension::Cells(0.),
                            bottom: Dimension::Cells(0.2),
                        }
                    } else {
                        BoxDimension {
                            left: Dimension::Cells(0.),
                            right: Dimension::Cells(0.),
                            top: Dimension::Cells(0.2),
                            bottom: Dimension::Cells(0.),
                        }
                    })
                    .padding(BoxDimension {
                        left: Dimension::Cells(0.5),
                        right: Dimension::Cells(0.5),
                        top: Dimension::Cells(0.25),
                        bottom: Dimension::Cells(0.25),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(if is_bottom {
                        Corners {
                            top_left: SizedPoly::none(),
                            top_right: SizedPoly::none(),
                            bottom_left: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: BOTTOM_LEFT_ROUNDED_CORNER,
                            },
                            bottom_right: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: BOTTOM_RIGHT_ROUNDED_CORNER,
                            },
                        }
                    } else {
                        Corners {
                            top_left: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: TOP_LEFT_ROUNDED_CORNER,
                            },
                            top_right: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: TOP_RIGHT_ROUNDED_CORNER,
                            },
                            bottom_left: SizedPoly::none(),
                            bottom_right: SizedPoly::none(),
                        }
                    }))
                    .colors(ElementColors {
                        border: BorderColor::new(
                            bg_color
                                .unwrap_or_else(|| active_tab.bg_color.into())
                                .to_linear(),
                        ),
                        bg: bg_color
                            .unwrap_or_else(|| active_tab.bg_color.into())
                            .to_linear()
                            .into(),
                        text: fg_color
                            .unwrap_or_else(|| active_tab.fg_color.into())
                            .to_linear()
                            .into(),
                    }),
                TabBarItem::Tab { .. } => element
                    .vertical_align(tab_vertical_alignment)
                    .item_type(UIItemType::TabBar(item.item.clone()))
                    .margin(if is_bottom {
                        BoxDimension {
                            left: Dimension::Cells(0.),
                            right: Dimension::Cells(0.),
                            top: Dimension::Cells(0.),
                            bottom: Dimension::Cells(0.2),
                        }
                    } else {
                        BoxDimension {
                            left: Dimension::Cells(0.),
                            right: Dimension::Cells(0.),
                            top: Dimension::Cells(0.2),
                            bottom: Dimension::Cells(0.),
                        }
                    })
                    .padding(BoxDimension {
                        left: Dimension::Cells(0.5),
                        right: Dimension::Cells(0.5),
                        top: Dimension::Cells(0.25),
                        bottom: Dimension::Cells(0.25),
                    })
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(if is_bottom {
                        Corners {
                            top_left: SizedPoly {
                                width: Dimension::Cells(0.),
                                height: Dimension::Cells(0.33),
                                poly: &[],
                            },
                            top_right: SizedPoly {
                                width: Dimension::Cells(0.),
                                height: Dimension::Cells(0.33),
                                poly: &[],
                            },
                            bottom_left: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: BOTTOM_LEFT_ROUNDED_CORNER,
                            },
                            bottom_right: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: BOTTOM_RIGHT_ROUNDED_CORNER,
                            },
                        }
                    } else {
                        Corners {
                            top_left: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: TOP_LEFT_ROUNDED_CORNER,
                            },
                            top_right: SizedPoly {
                                width: Dimension::Cells(0.5),
                                height: Dimension::Cells(0.5),
                                poly: TOP_RIGHT_ROUNDED_CORNER,
                            },
                            bottom_left: SizedPoly {
                                width: Dimension::Cells(0.),
                                height: Dimension::Cells(0.33),
                                poly: &[],
                            },
                            bottom_right: SizedPoly {
                                width: Dimension::Cells(0.),
                                height: Dimension::Cells(0.33),
                                poly: &[],
                            },
                        }
                    }))
                    .colors({
                        let inactive_tab = colors.inactive_tab();
                        let bg = bg_color
                            .unwrap_or_else(|| inactive_tab.bg_color.into())
                            .to_linear();
                        let edge = colors.inactive_tab_edge().to_linear();
                        ElementColors {
                            border: BorderColor {
                                left: bg,
                                right: edge,
                                top: bg,
                                bottom: bg,
                            },
                            bg: bg.into(),
                            text: fg_color
                                .unwrap_or_else(|| inactive_tab.fg_color.into())
                                .to_linear()
                                .into(),
                        }
                    })
                    .hover_colors({
                        let inactive_tab_hover = colors.inactive_tab_hover();
                        Some(ElementColors {
                            border: BorderColor::new(
                                bg_color
                                    .unwrap_or_else(|| inactive_tab_hover.bg_color.into())
                                    .to_linear(),
                            ),
                            bg: bg_color
                                .unwrap_or_else(|| inactive_tab_hover.bg_color.into())
                                .to_linear()
                                .into(),
                            text: fg_color
                                .unwrap_or_else(|| inactive_tab_hover.fg_color.into())
                                .to_linear()
                                .into(),
                        })
                    }),
                TabBarItem::WindowButton(button) => {
                    let element = window_button_element(
                        button,
                        self.window_state.contains(window::WindowState::MAXIMIZED),
                        &font,
                        &metrics,
                        &self.config,
                        scale,
                    );
                    if !chrome {
                        return element;
                    }
                    // in the strip's 40 visible DIP: a GTK picture where
                    // window_button_element put it (as Chrome's header
                    // bar would), a drawn button centred
                    let element = if self.config.integrated_title_button_images.is_empty() {
                        element.vertical_align(VerticalAlign::Middle)
                    } else {
                        element
                    };
                    Element::new(&font, ElementContent::Children(vec![element]))
                        .vertical_align(VerticalAlign::Top)
                        .min_height(Some(Dimension::Pixels(layout.height)))
                }
            }
        };

        let num_tabs: f32 = items
            .iter()
            .map(|item| match item.item {
                TabBarItem::NewTabButton | TabBarItem::Tab { .. } => 1.,
                TabBarItem::TabSearchButton => 0.,
                _ => 0.,
            })
            .sum();
        let max_tab_width = ((self.dimensions.pixel_width as f32 / num_tabs)
            - (1.5 * metrics.cell_size.width as f32))
            .max(0.);

        // Reserve space for the native titlebar buttons
        if self
            .config
            .window_decorations
            .contains(::window::WindowDecorations::INTEGRATED_BUTTONS)
            && self.config.integrated_title_button_style == IntegratedTitleButtonStyle::MacOsNative
            && !self.window_state.contains(window::WindowState::FULL_SCREEN)
        {
            left_status.push(
                Element::new(&font, ElementContent::Text("".to_string())).margin(BoxDimension {
                    left: Dimension::Cells(4.0), // FIXME: determine exact width of macos ... buttons
                    right: Dimension::Cells(0.),
                    top: Dimension::Cells(0.),
                    bottom: Dimension::Cells(0.),
                }),
            );
        }

        let (left_buttons, _) = self.config.integrated_title_button_sides();
        let mut first_tab = true;
        for item in items {
            match item.item {
                TabBarItem::LeftStatus => left_status.push(item_to_elem(item)),
                TabBarItem::TabSearchButton => {
                    if chrome {
                        left_eles.push(item_to_elem(item));
                    }
                }
                TabBarItem::None | TabBarItem::RightStatus => right_eles.push(item_to_elem(item)),
                TabBarItem::WindowButton(button) => {
                    if left_buttons.contains(&button) {
                        left_eles.push(item_to_elem(item))
                    } else {
                        right_eles.push(item_to_elem(item))
                    }
                }
                TabBarItem::Tab { tab_idx, active } => {
                    if chrome && first_tab {
                        // Chrome's strip starts at the header bar's padding
                        // (or after the buttons there), each tab carrying
                        // its feet: the first body that far in, clear of
                        // the window's round corner
                        // (Chrome: the body 12 DIP in, or past the
                        // buttons by the larger of 12 and the last
                        // button's own margin)
                        let buttons: f32 = layout
                            .left_placed
                            .iter()
                            .map(|b| b.width + b.margin_left + b.margin_right)
                            .sum();
                        // (or after the tab search button, when it stands there)
                        let before = layout
                            .tab_search
                            .filter(|_| !layout.tab_search_trailing)
                            .map_or(buttons, |b| b.right());
                        let body = layout.tabs.first().map_or(0., |t| t.body.x);
                        left_eles.push(
                            Element::new(&font, ElementContent::Text(String::new())).min_width(
                                Some(Dimension::Pixels(
                                    (body - before - dip_px(chrome_tabs::GAP / 2.)).max(0.),
                                )),
                            ),
                        );
                    }
                    first_tab = false;
                    let t = layout.tabs.get(tab_no).cloned();
                    tab_no += 1;
                    // a tab the strip has no room for is not drawn at all
                    if chrome && t.as_ref().is_some_and(|t| !t.visible) {
                        continue;
                    }
                    let mut elem = item_to_elem(item);
                    elem.max_width = Some(Dimension::Pixels(match &t {
                        Some(t) if chrome => t.body.w,
                        _ => max_tab_width,
                    }));
                    elem.content = match elem.content {
                        ElementContent::Text(_) => unreachable!(),
                        ElementContent::Poly { .. }
                        | ElementContent::Icon { .. }
                        | ElementContent::Image { .. } => unreachable!(),
                        ElementContent::Children(mut kids) => {
                            if let Some(t) = t.as_ref().filter(|_| chrome) {
                                // the title ends where the close button
                                // begins (it floats over whatever is there)
                                let room = t.title_width;
                                let title = kids
                                    .into_iter()
                                    .map(|k| k.vertical_align(VerticalAlign::Middle))
                                    .collect();
                                // the title's baseline where Chrome's label
                                // puts it, within the 28-DIP row
                                let title_top = t.title_top;
                                // the favicon before it, as Chrome's
                                let icon = t.favicon.map(|favicon| {
                                    let path = self.config.tab_icon.clone().unwrap_or_default();
                                    let size = Dimension::Pixels(favicon.w);
                                    Element::new(
                                        &font,
                                        ElementContent::Image {
                                            normal: path.clone(),
                                            hover: None,
                                            backdrop: None,
                                            width: size,
                                            height: size,
                                        },
                                    )
                                    .vertical_align(VerticalAlign::Middle)
                                    .margin(BoxDimension {
                                        left: dip(0.),
                                        right: Dimension::Pixels(t.title_left - favicon.right()),
                                        top: dip(0.),
                                        bottom: dip(0.),
                                    })
                                });
                                kids = icon.into_iter().collect();
                                kids.push(
                                    Element::new(&font, ElementContent::Children(title))
                                        .vertical_align(VerticalAlign::Top)
                                        .margin(BoxDimension {
                                            left: dip(0.),
                                            right: dip(0.),
                                            top: Dimension::Pixels(title_top),
                                            bottom: dip(0.),
                                        })
                                        .max_width(Some(Dimension::Pixels(room.max(0.)))),
                                );
                            }
                            if let Some(t) = t.as_ref().filter(|_| chrome) {
                                // Chrome's close button: shown by the tab's
                                // width and state, highlighted by the colour
                                // that contrasts most with the tab
                                if let (Some(c), Some(icon)) = (t.close, t.close_icon) {
                                    let bg: config::SrgbaTuple = if active {
                                        colors.active_tab().bg_color.into()
                                    } else {
                                        frame_colour
                                    };
                                    let fg = if active {
                                        colors.active_tab().fg_color
                                    } else {
                                        colors.inactive_tab().fg_color
                                    }
                                    .to_linear();
                                    kids.push(make_chrome_x_button(
                                        &font,
                                        &metrics,
                                        tab_idx,
                                        fg,
                                        chrome_tabs::max_contrast(bg)
                                            .to_linear()
                                            .mul_alpha(chrome_tabs::HIGHLIGHT),
                                        icon.w,
                                        (c.w - icon.w) / 2.,
                                        dip_px(chrome_tabs::CLOSE_HIGHLIGHT_RADIUS),
                                    ));
                                }
                            } else if self.config.show_close_tab_button_in_tabs {
                                kids.push(make_x_button(&font, &metrics, &colors, tab_idx, active));
                            }
                            ElementContent::Children(kids)
                        }
                    };
                    left_eles.push(elem);
                }
                _ => left_eles.push(item_to_elem(item)),
            }
        }

        let mut children = vec![];

        if !left_status.is_empty() {
            children.push(
                Element::new(&font, ElementContent::Children(left_status))
                    .colors(bar_colors.clone()),
            );
        }

        let window_buttons_at_left = self.config.caption_buttons_lead();

        let left_padding = if window_buttons_at_left {
            if self.config.integrated_title_button_style == IntegratedTitleButtonStyle::MacOsNative
            {
                if !self.window_state.contains(window::WindowState::FULL_SCREEN) {
                    Dimension::Pixels(70.0)
                } else {
                    Dimension::Cells(0.5)
                }
            } else {
                Dimension::Pixels(0.0)
            }
        } else if chrome {
            Dimension::Pixels(0.0)
        } else {
            Dimension::Cells(0.5)
        };

        children.push(
            Element::new(&font, ElementContent::Children(left_eles))
                .vertical_align(tab_vertical_alignment)
                .colors(bar_colors.clone())
                .padding(BoxDimension {
                    left: left_padding,
                    right: Dimension::Cells(0.),
                    top: Dimension::Cells(0.),
                    bottom: Dimension::Cells(0.),
                })
                .zindex(1),
        );
        children.push(
            Element::new(&font, ElementContent::Children(right_eles))
                .colors(bar_colors.clone())
                .float(Float::Right),
        );

        let content = ElementContent::Children(children);

        let tabs = Element::new(&font, content)
            .display(DisplayType::Block)
            .item_type(UIItemType::TabBar(TabBarItem::None))
            .min_width(Some(Dimension::Pixels(self.dimensions.pixel_width as f32)))
            .min_height(Some(Dimension::Pixels(tab_bar_height)))
            .vertical_align(tab_vertical_alignment)
            .colors(bar_colors);

        let border = self.get_os_border();

        let mut computed = self.compute_element(
            &LayoutContext {
                height: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_height as f32,
                    pixel_cell: metrics.cell_size.height as f32,
                },
                width: DimensionContext {
                    dpi: self.dimensions.dpi as f32,
                    pixel_max: self.dimensions.pixel_width as f32,
                    pixel_cell: metrics.cell_size.width as f32,
                },
                bounds: euclid::rect(
                    border.left.get() as f32,
                    0.,
                    self.dimensions.pixel_width as f32 - (border.left + border.right).get() as f32,
                    tab_bar_height,
                ),
                metrics: &metrics,
                gl_state: self.render_state.as_ref().unwrap(),
                zindex: 10,
            },
            &tabs,
        )?;

        computed.translate(euclid::vec2(
            0.,
            if self.config.tab_bar_at_bottom {
                self.dimensions.pixel_height as f32
                    - (computed.bounds.height() + border.bottom.get() as f32)
            } else {
                border.top.get() as f32
            },
        ));

        Ok(computed)
    }

    pub fn paint_fancy_tab_bar(&self) -> anyhow::Result<Vec<UIItem>> {
        let computed = self.fancy_tab_bar.as_ref().ok_or_else(|| {
            anyhow::anyhow!("paint_fancy_tab_bar called but fancy_tab_bar is None")
        })?;
        let ui_items = computed.ui_items();
        let gl_state = self.render_state.as_ref().unwrap();
        if self.config.tab_strip_style == config::TabStripStyle::Chrome {
            self.paint_chrome_tabs(computed)?;
        }
        self.render_element(&computed, gl_state, None)?;

        Ok(ui_items)
    }
}

impl crate::TermWindow {
    /// Chrome's strip laid out for this window: its width and scale, the
    /// tabs, the desktop's caption buttons, the title font.
    pub(crate) fn chrome_layout(
        &self,
        font: &Rc<LoadedFont>,
        metrics: &RenderMetrics,
    ) -> anyhow::Result<chrome_strip::Layout> {
        let images = &self.config.integrated_title_button_images;
        let image = |button: &IntegratedTitleButton| {
            let key = match button {
                IntegratedTitleButton::Hide => "minimize",
                IntegratedTitleButton::Maximize => "maximize",
                IntegratedTitleButton::Close => "close",
            };
            images.get(key).map(|i| chrome_strip::ButtonImage {
                width: i.width as f32,
                height: i.height as f32,
                margin_left: i.margin_left as f32,
                margin_right: i.margin_right as f32,
                margin_top: i.margin_top as f32,
                margin_bottom: i.margin_bottom as f32,
                header_top: i.header_top as f32,
                header_bottom: i.header_bottom as f32,
            })
        };
        let (left, right) = self.config.integrated_title_button_sides();
        let fm = font.metrics();
        let cell = metrics.cell_size.height as f32;
        let descender = metrics.descender.get() as f32;
        Ok(chrome_strip::Layout::compute(&chrome_strip::Inputs {
            scale: self.dimensions.dpi as f32 / 96.,
            width_px: self.dimensions.pixel_width as f32,
            tabs: self
                .tab_bar
                .items()
                .iter()
                .filter(|i| matches!(i.item, TabBarItem::Tab { .. }))
                .count(),
            active: self
                .tab_bar
                .items()
                .iter()
                .filter(|i| matches!(i.item, TabBarItem::Tab { .. }))
                .position(|i| matches!(i.item, TabBarItem::Tab { active: true, .. }))
                .unwrap_or(0),
            left_buttons: left.iter().filter_map(image).collect(),
            right_buttons: right.iter().filter_map(image).collect(),
            // WezTerm's own drawn buttons, when the desktop gave none
            drawn_buttons: if images.is_empty() { 138. } else { 0. },
            close_buttons: self.config.show_close_tab_button_in_tabs,
            tab_search: true,
            tab_search_trailing: self.config.caption_buttons_lead(),
            favicons: self.config.tab_icon.is_some(),
            font: chrome_strip::FontMetrics {
                ascent: (fm.cell_height.get() + fm.descender.get()) as f32,
                descent: -fm.descender.get() as f32,
                cap_height: fm.cap_height.map(|c| c.get() as f32),
                cell_height: cell,
                baseline_in_cell: cell + descender,
            },
        }))
    }
}

/// Chrome's tab strip beneath the elements: the strip in the frame's
/// colour, separators between the unfilled tabs, the tab under the
/// pointer filled lightly, the active tab filled with its feet and, where
/// it would not stand out, stroked.
impl crate::TermWindow {
    fn paint_chrome_tabs(&self, computed: &ComputedElement) -> anyhow::Result<()> {
        let _timer = crate::stats::Timed::new("chrome_tabs.paint");
        let scale = self.dimensions.dpi as f32 / 96.;
        let focused = self.focused.is_some();
        let frame: config::SrgbaTuple = if focused {
            self.config.window_frame.active_titlebar_bg
        } else {
            self.config.window_frame.inactive_titlebar_bg
        }
        .into();
        let frame_active: config::SrgbaTuple = self.config.window_frame.active_titlebar_bg.into();
        let colors = self
            .config
            .colors
            .as_ref()
            .and_then(|c| c.tab_bar.as_ref())
            .cloned()
            .unwrap_or_else(TabBarColors::default);
        let active: config::SrgbaTuple = colors.active_tab().bg_color.into();
        let lin = |c: config::SrgbaTuple| c.to_linear();

        let gl_state = self.render_state.as_ref().unwrap();
        let layer = gl_state.layer_for_zindex(computed.zindex)?;
        let mut layers = layer.quad_allocator();
        let strip = computed.bounds;
        self.filled_rectangle(&mut layers, 0, strip, lin(frame))?;

        let font = self.fonts.title_font()?;
        let metrics = RenderMetrics::with_font_metrics(&font.metrics());
        let layout = self.chrome_layout(&font, &metrics)?;
        // the bodies as the elements were laid out (the shown tabs, in
        // order), each with its own numbers from the layout
        let bodies = chrome_tabs::tabs(computed);
        let shown: Vec<&chrome_strip::Tab> = layout.tabs.iter().filter(|t| t.visible).collect();
        let condensed = self.window_state.contains(window::WindowState::MAXIMIZED);
        let mouse = self
            .current_mouse_event
            .as_ref()
            .map(|e| (e.coords.x as f32, e.coords.y as f32));
        // Chrome's hit test: the body and 3 DIP into each separator, the
        // tab's row (to the top when maximized)
        let reach = chrome_strip::dip(3., scale);
        let hovered = |r: &RectF| {
            mouse.is_some_and(|(x, y)| {
                let top = if condensed { strip.min_y() } else { r.min_y() };
                x >= r.min_x() - reach && x < r.max_x() + reach && y >= top && y < strip.max_y()
            })
        };

        // the hover fills come and go over 200 ms
        let now_ms = self.chrome_hover.borrow().base.elapsed().as_secs_f64() * 1000.;
        let values: Vec<f32> = {
            let mut state = self.chrome_hover.borrow_mut();
            state.tabs.resize(bodies.len(), chrome_tabs::Hover::at(0.));
            let mut running = false;
            let values = bodies
                .iter()
                .enumerate()
                .map(|(i, (r, is_active))| {
                    let target = if !is_active && hovered(r) { 1. } else { 0. };
                    state.tabs[i] = state.tabs[i].towards(target, now_ms);
                    running |= !state.tabs[i].done(now_ms);
                    state.tabs[i].value(now_ms)
                })
                .collect();
            if running {
                self.update_next_frame_time(Some(
                    std::time::Instant::now() + std::time::Duration::from_millis(16),
                ));
            }
            values
        };

        // the toolbar's first row under the strip: the tabs' colour, or the
        // top line where the active tab needs its stroke
        let stroke = chrome_tabs::stroke(active, frame_active);
        let line = euclid::rect(
            strip.min_x(),
            strip.min_y() + layout.top_line,
            strip.width(),
            strip.height() - layout.top_line,
        );
        self.filled_rectangle(&mut layers, 0, line, lin(stroke.unwrap_or(active)))?;
        // Chrome's solid frame: its border line's top run, across the
        // strip's first row (the sides and bottom are the window's own
        // margins)
        if self.window_state.contains(window::WindowState::SOLID_EDGE) {
            let line = chrome_strip::frame::solid_border(frame);
            let top = euclid::rect(strip.min_x(), strip.min_y(), strip.width(), 1.);
            let over = chrome_strip::blend(line, frame, line.3);
            self.filled_rectangle(&mut layers, 0, top, lin(over))?;
        }

        // separators: after each tab (Chrome's trailing one; the next tab's
        // leading one stands in the same place), fading with the hovers
        for (i, (r, is_active)) in bodies.iter().enumerate() {
            let next = bodies.get(i + 1);
            let opacity = chrome_tabs::separator_opacity(
                *is_active,
                values[i],
                next.map_or(false, |(_, a)| *a),
                next.map_or(0., |_| values[i + 1]),
            );
            if opacity <= 0. {
                continue;
            }
            let Some(t) = shown.get(i) else { break };
            let s = t.trailing_separator;
            self.tab_shape_quad(
                &mut layers,
                0,
                euclid::rect(
                    r.max_x() + (s.x - t.body.right()),
                    strip.min_y() + s.y,
                    s.w,
                    s.h,
                ),
                chrome_tabs::SEPARATOR_RADIUS * scale,
                0.,
                false,
                lin(active).mul_alpha(opacity),
            )?;
        }

        // the hover fills: the theme's colour (or Chrome's baseline) over
        // the strip by the animation
        let theme_hover: Option<config::SrgbaTuple> = colors
            .inactive_tab_hover
            .as_ref()
            .map(|c| c.bg_color.into());
        for (i, (r, is_active)) in bodies.iter().enumerate() {
            if *is_active || values[i] <= 0. {
                continue;
            }
            let Some(t) = shown.get(i) else { break };
            let fill = chrome_tabs::hover_fill(theme_hover, frame, focused);
            let colour = chrome_tabs::hover_at(fill, frame, values[i]);
            self.tab_shape_quad(
                &mut layers,
                0,
                euclid::rect(r.min_x(), r.min_y(), r.width(), t.highlight.h),
                t.radius,
                0.,
                false,
                lin(colour),
            )?;
        }

        // the active tab: its fill with its feet down to the content, and
        // its stroke where the strip would not show it
        if let Some((i, (r, _))) = bodies.iter().enumerate().find(|(_, (_, a))| *a) {
            let radius = layout
                .tabs
                .get(i)
                .map_or(chrome_tabs::TOP_RADIUS * scale, |t| t.radius);
            let foot = chrome_strip::dip(chrome_tabs::FOOT, scale);
            let shape = euclid::rect(
                r.min_x() - foot,
                r.min_y(),
                r.width() + 2. * foot,
                strip.max_y() - r.min_y(),
            );
            self.tab_shape_quad(&mut layers, 0, shape, radius, foot, false, lin(active))?;
            if let Some(stroke) = stroke {
                self.tab_shape_quad(&mut layers, 0, shape, radius, foot, true, lin(stroke))?;
            }
        }
        Ok(())
    }
}

/// The strip's hover animations, one per tab, timed from `base`.
#[derive(Debug)]
pub struct ChromeHover {
    pub base: std::time::Instant,
    pub tabs: Vec<chrome_tabs::Hover>,
}

impl Default for ChromeHover {
    fn default() -> ChromeHover {
        ChromeHover {
            base: std::time::Instant::now(),
            tabs: Vec::new(),
        }
    }
}

/// Chrome's tab close button: a cross 8 DIP across in a 16-DIP icon,
/// which a circle of radius 8 highlights under the pointer; the button
/// around it, 28 DIP, is what the pointer hits.
fn make_chrome_x_button(
    font: &Rc<LoadedFont>,
    metrics: &RenderMetrics,
    tab_idx: usize,
    fg: LinearRgba,
    highlight: LinearRgba,
    icon: f32,
    margin: f32,
    radius: f32,
) -> Element {
    let cross = (icon / 2.).round();
    Element::new(
        &font,
        ElementContent::Poly {
            line_width: metrics.underline_height.max(2),
            poly: SizedPoly {
                poly: X_BUTTON,
                width: Dimension::Pixels(cross),
                height: Dimension::Pixels(cross),
            },
        },
    )
    .zindex(1)
    .vertical_align(VerticalAlign::Middle)
    .float(Float::Right)
    .item_type(UIItemType::CloseTab(tab_idx))
    .margin(BoxDimension::new(Dimension::Pixels(margin)))
    .padding(BoxDimension::new(Dimension::Pixels(
        ((icon - cross) / 2.).max(0.).floor(),
    )))
    .border(BoxDimension::new(Dimension::Pixels(0.)))
    .border_corners(Some(chrome_circle(radius)))
    .colors(ElementColors {
        border: BorderColor::new(window::color::LinearRgba::TRANSPARENT),
        bg: window::color::LinearRgba::TRANSPARENT.into(),
        text: fg.into(),
    })
    .hover_colors(Some(ElementColors {
        border: BorderColor::new(highlight),
        bg: highlight.into(),
        text: fg.into(),
    }))
}

/// The rounded corners that make a box `radius` pixels round.
fn chrome_circle(radius: f32) -> Corners {
    let r = Dimension::Pixels(radius.round());
    let corner = |poly| SizedPoly {
        width: r,
        height: r,
        poly,
    };
    Corners {
        top_left: corner(TOP_LEFT_ROUNDED_CORNER),
        top_right: corner(TOP_RIGHT_ROUNDED_CORNER),
        bottom_left: corner(BOTTOM_LEFT_ROUNDED_CORNER),
        bottom_right: corner(BOTTOM_RIGHT_ROUNDED_CORNER),
    }
}

fn make_x_button(
    font: &Rc<LoadedFont>,
    metrics: &RenderMetrics,
    colors: &TabBarColors,
    tab_idx: usize,
    active: bool,
) -> Element {
    Element::new(
        &font,
        ElementContent::Poly {
            line_width: metrics.underline_height.max(2),
            poly: SizedPoly {
                poly: X_BUTTON,
                width: Dimension::Pixels(metrics.cell_size.height as f32 / 2.),
                height: Dimension::Pixels(metrics.cell_size.height as f32 / 2.),
            },
        },
    )
    // Ensure that we draw our background over the
    // top of the rest of the tab contents
    .zindex(1)
    .vertical_align(VerticalAlign::Middle)
    .float(Float::Right)
    .item_type(UIItemType::CloseTab(tab_idx))
    .hover_colors({
        let inactive_tab_hover = colors.inactive_tab_hover();
        let active_tab = colors.active_tab();

        Some(ElementColors {
            border: BorderColor::default(),
            bg: (if active {
                inactive_tab_hover.bg_color
            } else {
                active_tab.bg_color
            })
            .to_linear()
            .into(),
            text: (if active {
                inactive_tab_hover.fg_color
            } else {
                active_tab.fg_color
            })
            .to_linear()
            .into(),
        })
    })
    .padding(BoxDimension {
        left: Dimension::Cells(0.25),
        right: Dimension::Cells(0.25),
        top: Dimension::Cells(0.25),
        bottom: Dimension::Cells(0.25),
    })
    .margin(BoxDimension {
        left: Dimension::Cells(0.5),
        right: Dimension::Cells(0.),
        top: Dimension::Cells(0.),
        bottom: Dimension::Cells(0.),
    })
}
