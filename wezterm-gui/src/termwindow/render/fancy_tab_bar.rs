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
use window::IntegratedTitleButtonStyle;
use window::RectF;

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
    }

    pub fn build_fancy_tab_bar(&self, palette: &ColorPalette) -> anyhow::Result<ComputedElement> {
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
        let chrome_body = {
            let tabs = items
                .iter()
                .filter(|i| matches!(i.item, TabBarItem::Tab { .. }))
                .count();
            // the room left by the window buttons and the new-tab button
            let buttons: f32 = self
                .config
                .integrated_title_button_images
                .values()
                .map(|b| (b.width + b.margin_left + b.margin_right) as f32)
                .sum::<f32>()
                .max(if self.config.integrated_title_button_images.is_empty() {
                    138.
                } else {
                    0.
                });
            let room = self.dimensions.pixel_width as f32
                - (buttons + chrome_tabs::BUTTON + 2. * chrome_tabs::FOOT + 12.) * scale;
            chrome_tabs::body_width(room, tabs, scale)
        };
        let tab_vertical_alignment = if self.config.tab_bar_at_bottom {
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
                TabBarItem::NewTabButton if chrome => {
                    let fg = new_tab.fg_color.to_linear();
                    Element::new(
                        &font,
                        ElementContent::Poly {
                            line_width: metrics.underline_height.max(2),
                            poly: SizedPoly {
                                poly: PLUS_BUTTON,
                                width: dip(10.),
                                height: dip(10.),
                            },
                        },
                    )
                    .vertical_align(VerticalAlign::Middle)
                    .item_type(UIItemType::TabBar(item.item.clone()))
                    .margin(BoxDimension {
                        left: dip(2.),
                        right: dip(0.),
                        top: dip((chrome_tabs::STRIP_HEIGHT - chrome_tabs::BUTTON) / 2.),
                        bottom: dip(0.),
                    })
                    .padding(BoxDimension::new(
                        dip((chrome_tabs::BUTTON - 10.) / 2. - 1.),
                    ))
                    .border(BoxDimension::new(Dimension::Pixels(1.)))
                    .border_corners(Some(chrome_circle(scale)))
                    .colors(ElementColors {
                        border: BorderColor::new(window::color::LinearRgba::TRANSPARENT),
                        bg: window::color::LinearRgba::TRANSPARENT.into(),
                        text: fg.into(),
                    })
                    .hover_colors(Some(ElementColors {
                        border: BorderColor::new(fg.mul_alpha(chrome_tabs::HIGHLIGHT)),
                        bg: fg.mul_alpha(chrome_tabs::HIGHLIGHT).into(),
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
                TabBarItem::Tab { active, .. } if chrome => {
                    let tab = if active {
                        active_tab.clone()
                    } else {
                        colors.inactive_tab()
                    };
                    let clear = window::color::LinearRgba::TRANSPARENT;
                    let fg = fg_color.unwrap_or_else(|| tab.fg_color.into()).to_linear();
                    // the line (the close button's 28, or the title's own
                    // height) centred in the shape, above its 1-DIP
                    // overlap; the title centred on that line
                    let body = (chrome_tabs::STRIP_HEIGHT - chrome_tabs::TAB_TOP - 1.) * scale;
                    let line = if self.config.show_close_tab_button_in_tabs {
                        (chrome_tabs::BUTTON * scale).max(metrics.cell_size.height as f32)
                    } else {
                        metrics.cell_size.height as f32
                    };
                    let pad = ((body - line) / 2.).max(0.).floor();
                    element
                        .vertical_align(VerticalAlign::Top)
                        .item_type(UIItemType::TabBar(item.item.clone()))
                        .margin(BoxDimension {
                            left: dip(chrome_tabs::GAP / 2.),
                            right: dip(chrome_tabs::GAP / 2.),
                            top: dip(chrome_tabs::TAB_TOP),
                            bottom: dip(0.),
                        })
                        .padding(BoxDimension {
                            left: dip(chrome_tabs::INSET),
                            right: dip(chrome_tabs::INSET / 2.),
                            top: Dimension::Pixels(pad),
                            bottom: dip(0.),
                        })
                        .min_width(Some(Dimension::Pixels(chrome_body)))
                        .min_height(Some(dip(chrome_tabs::STRIP_HEIGHT - chrome_tabs::TAB_TOP)))
                        .border(BoxDimension::new(Dimension::Pixels(0.)))
                        .colors(ElementColors {
                            border: BorderColor::new(clear),
                            bg: clear.into(),
                            text: fg.into(),
                        })
                        .hover_colors(Some(ElementColors {
                            border: BorderColor::new(clear),
                            bg: clear.into(),
                            text: fg.into(),
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
                TabBarItem::WindowButton(button) => window_button_element(
                    button,
                    self.window_state.contains(window::WindowState::MAXIMIZED),
                    &font,
                    &metrics,
                    &self.config,
                ),
            }
        };

        let num_tabs: f32 = items
            .iter()
            .map(|item| match item.item {
                TabBarItem::NewTabButton | TabBarItem::Tab { .. } => 1.,
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
        for item in items {
            match item.item {
                TabBarItem::LeftStatus => left_status.push(item_to_elem(item)),
                TabBarItem::None | TabBarItem::RightStatus => right_eles.push(item_to_elem(item)),
                TabBarItem::WindowButton(button) => {
                    if left_buttons.contains(&button) {
                        left_eles.push(item_to_elem(item))
                    } else {
                        right_eles.push(item_to_elem(item))
                    }
                }
                TabBarItem::Tab { tab_idx, active } => {
                    let mut elem = item_to_elem(item);
                    elem.max_width = Some(Dimension::Pixels(if chrome {
                        chrome_body
                    } else {
                        max_tab_width
                    }));
                    elem.content = match elem.content {
                        ElementContent::Text(_) => unreachable!(),
                        ElementContent::Poly { .. }
                        | ElementContent::Icon { .. }
                        | ElementContent::Image { .. } => unreachable!(),
                        ElementContent::Children(mut kids) => {
                            if chrome {
                                // the title ends where the close button
                                // begins (it floats over whatever is there)
                                let mut room = chrome_body - 1.5 * chrome_tabs::INSET * scale;
                                if self.config.show_close_tab_button_in_tabs {
                                    room -= chrome_tabs::BUTTON * scale;
                                }
                                let title = kids
                                    .into_iter()
                                    .map(|k| k.vertical_align(VerticalAlign::Middle))
                                    .collect();
                                kids = vec![Element::new(&font, ElementContent::Children(title))
                                    .vertical_align(VerticalAlign::Middle)
                                    .max_width(Some(Dimension::Pixels(room.max(0.))))];
                            }
                            if self.config.show_close_tab_button_in_tabs {
                                kids.push(if chrome {
                                    make_chrome_x_button(
                                        &font, &metrics, &colors, tab_idx, active, scale,
                                    )
                                } else {
                                    make_x_button(&font, &metrics, &colors, tab_idx, active)
                                });
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

        let window_buttons_at_left = self
            .config
            .window_decorations
            .contains(window::WindowDecorations::INTEGRATED_BUTTONS)
            && (!left_buttons.is_empty()
                || self.config.integrated_title_button_style
                    == IntegratedTitleButtonStyle::MacOsNative);

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

/// Chrome's tab strip beneath the elements: the strip in the frame's
/// colour, separators between the unfilled tabs, the tab under the
/// pointer filled lightly, the active tab filled with its feet and, where
/// it would not stand out, stroked.
impl crate::TermWindow {
    fn paint_chrome_tabs(&self, computed: &ComputedElement) -> anyhow::Result<()> {
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

        let mouse = self
            .current_mouse_event
            .as_ref()
            .map(|e| (e.coords.x as f32, e.coords.y as f32));
        let tabs = chrome_tabs::tabs(computed);
        let hovered = |r: &RectF| {
            mouse.is_some_and(|(x, y)| {
                x >= r.min_x() && x < r.max_x() && y >= strip.min_y() && y < strip.max_y()
            })
        };
        let foot = chrome_tabs::FOOT * scale;
        let shape_of = |r: &RectF| {
            euclid::rect(
                r.min_x() - foot,
                r.min_y(),
                r.width() + 2. * foot,
                strip.max_y() - r.min_y(),
            )
        };

        // separators between two unfilled tabs
        let (sw, sh) = (
            chrome_tabs::SEPARATOR.0 * scale,
            chrome_tabs::SEPARATOR.1 * scale,
        );
        for pair in tabs.windows(2) {
            let ((a, a_active), (b, b_active)) = (pair[0], pair[1]);
            if a_active || b_active || hovered(&a) || hovered(&b) {
                continue;
            }
            let x = ((a.max_x() + b.min_x() - sw) / 2.).round();
            let y = (a.min_y() + (a.height() - sh) / 2.).round();
            self.filled_rectangle(&mut layers, 0, euclid::rect(x, y, sw, sh), lin(active))?;
        }

        let top = chrome_tabs::TOP_RADIUS * scale;
        for (r, is_active) in &tabs {
            if !is_active && hovered(r) {
                let fill = chrome_tabs::hover(active, frame);
                self.tab_shape_quad(&mut layers, 0, shape_of(r), top, foot, false, lin(fill))?;
            }
        }
        if let Some((r, _)) = tabs.iter().find(|(_, a)| *a) {
            self.tab_shape_quad(&mut layers, 0, shape_of(r), top, foot, false, lin(active))?;
            // compared with the focused frame, so it does not come and go
            // with the focus
            if let Some(stroke) = chrome_tabs::stroke(active, frame_active) {
                self.tab_shape_quad(&mut layers, 0, shape_of(r), top, foot, true, lin(stroke))?;
            }
        }
        Ok(())
    }
}

/// Chrome's tab close button: a small cross in a 28-DIP circle that
/// shows under the pointer.
fn make_chrome_x_button(
    font: &Rc<LoadedFont>,
    metrics: &RenderMetrics,
    colors: &TabBarColors,
    tab_idx: usize,
    active: bool,
    scale: f32,
) -> Element {
    let fg = if active {
        colors.active_tab().fg_color
    } else {
        colors.inactive_tab().fg_color
    }
    .to_linear();
    let icon = (8. * scale).round();
    Element::new(
        &font,
        ElementContent::Poly {
            line_width: metrics.underline_height.max(2),
            poly: SizedPoly {
                poly: X_BUTTON,
                width: Dimension::Pixels(icon),
                height: Dimension::Pixels(icon),
            },
        },
    )
    .zindex(1)
    .vertical_align(VerticalAlign::Middle)
    .float(Float::Right)
    .item_type(UIItemType::CloseTab(tab_idx))
    .padding(BoxDimension::new(Dimension::Pixels(
        ((chrome_tabs::BUTTON * scale - icon) / 2. - 1.)
            .max(0.)
            .round(),
    )))
    .border(BoxDimension::new(Dimension::Pixels(1.)))
    .border_corners(Some(chrome_circle(scale)))
    .colors(ElementColors {
        border: BorderColor::new(window::color::LinearRgba::TRANSPARENT),
        bg: window::color::LinearRgba::TRANSPARENT.into(),
        text: fg.into(),
    })
    .hover_colors(Some(ElementColors {
        border: BorderColor::new(fg.mul_alpha(chrome_tabs::HIGHLIGHT)),
        bg: fg.mul_alpha(chrome_tabs::HIGHLIGHT).into(),
        text: fg.into(),
    }))
}

/// The rounded corners that make a 28-DIP button a circle.
fn chrome_circle(scale: f32) -> Corners {
    let r = Dimension::Pixels((chrome_tabs::BUTTON / 2. * scale).round());
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
