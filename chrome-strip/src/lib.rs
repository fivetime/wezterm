//! Chromium's tab strip as it draws it over a Linux desktop theme: every
//! size, position and colour rule, computed here in pixels from the
//! inputs alone (the window's width, the scale, the tabs, the theme's
//! buttons, the title font's metrics), with nothing to draw them.
//!
//! The renderer takes its numbers from `Layout`; the tests here hold
//! them against Chromium's (layout_constants.cc, tab_style.cc,
//! horizontal_tab_style_views.cc, tab.cc, nav_button_provider_gtk.cc,
//! render_text.cc, tab_strip_color_mixer.cc), so a change is checked
//! against Chrome on any machine, not by looking at a desktop.
//!
//! Sizes are DIP (pixels at 96 dpi) unless named `_px`; a DIP becomes
//! pixels by `dip()`, rounded as Chrome rounds its layout to whole
//! pixels.

pub use wezterm_color_types::SrgbaTuple;

/// The strip is 41 DIP (kTabStripHeight = kTabHeight 35 + kTabStripPadding
/// 6), its last DIP under the toolbar (kTabstripToolbarOverlap): only
/// `VISIBLE` shows, and the content begins there.
pub const STRIP_HEIGHT: f32 = 41.;
pub const OVERLAP: f32 = 1.;
pub const VISIBLE: f32 = STRIP_HEIGHT - OVERLAP;
/// The tab shape starts this far below the strip's top (kTabStripPadding).
pub const TAB_TOP: f32 = 6.;
/// The hover highlight and the tab's contents: kTabHeight less the padding
/// and the overlap.
pub const HIGHLIGHT_HEIGHT: f32 = 28.;
/// TabStyle::GetTopCornerRadius, GetBottomCornerRadius (the feet).
pub const TOP_RADIUS: f32 = 10.;
pub const FOOT: f32 = 12.;
/// A tab's body between its feet: the standard width less the feet, and
/// the narrowest.
pub const BODY_MAX: f32 = 232.;
pub const BODY_MIN: f32 = 32.;
/// Between two bodies: 2 feet of 12 overlapping by 18.
pub const GAP: f32 = 6.;
/// The contents' inset from the body's edge (Chrome's 20 from the tab's).
pub const INSET: f32 = 8.;
/// TabStyle::GetSeparatorSize, its corner radius, and its row: centred on
/// the highlight.
pub const SEPARATOR: (f32, f32) = (2., 16.);
pub const SEPARATOR_RADIUS: f32 = 1.;
/// The close and new-tab buttons: a 28-DIP circle around a 16-DIP icon;
/// their highlight's opacity under the pointer.
pub const BUTTON: f32 = 28.;
pub const BUTTON_ICON: f32 = 16.;
pub const HIGHLIGHT_OPACITY: f32 = 0.16;
/// gfx::kFaviconSize and kTabPreTitlePadding after it.
pub const FAVICON: f32 = 16.;
pub const FAVICON_GAP: f32 = 8.;
/// The first tab's body from the client edge, or past the caption
/// buttons (the larger of this and their own margin).
pub const LEADING: f32 = 12.;
/// The strip's room after the last tab for the new-tab button.
pub const NEW_TAB_MARGIN: f32 = 2.;

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// A caption button picture as the desktop's GTK theme drew it, with its
/// CSS margins and the header bar's padding above and below it (DIP).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonImage {
    pub width: f32,
    pub height: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub header_top: f32,
    pub header_bottom: f32,
}

/// The title font's metrics in pixels at the scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    /// The cap height where the font tells it (OS/2 sCapHeight).
    pub cap_height: Option<f32>,
    /// The renderer's line height for the font (its cell), and where the
    /// baseline lies below the cell's top.
    pub cell_height: f32,
    pub baseline_in_cell: f32,
}

/// What the strip is laid out from.
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    /// Pixels per DIP.
    pub scale: f32,
    /// The window's width in pixels.
    pub width_px: f32,
    pub tabs: usize,
    /// The caption buttons on each side, in their order from the edge.
    pub left_buttons: Vec<ButtonImage>,
    pub right_buttons: Vec<ButtonImage>,
    /// The room drawn caption buttons take when there are no pictures.
    pub drawn_buttons: f32,
    pub close_buttons: bool,
    pub favicons: bool,
    pub font: FontMetrics,
}

impl Default for Inputs {
    fn default() -> Inputs {
        Inputs {
            scale: 1.,
            width_px: 1000.,
            tabs: 1,
            left_buttons: vec![],
            right_buttons: vec![],
            drawn_buttons: 0.,
            close_buttons: true,
            favicons: true,
            font: FontMetrics {
                ascent: 17.4,
                descent: 4.3,
                cap_height: Some(10.7),
                cell_height: 22.,
                baseline_in_cell: 17.,
            },
        }
    }
}

/// A caption button picture placed in the top area: its picture scaled
/// by `factor` to fit, its top at `top` px from the strip's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedButton {
    pub factor: f32,
    pub width: f32,
    pub height: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    pub top: f32,
}

/// The strip in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    /// The visible strip: the content begins at this row.
    pub height: f32,
    /// A tab's row (its highlight, and what it holds), below the strip's top.
    pub tab_top: f32,
    pub tab_height: f32,
    /// The gap kept below the tab row to the visible strip's bottom.
    pub tab_bottom_gap: f32,
    /// A tab body's width, the same for all.
    pub body_width: f32,
    /// The room before the first body: from the client edge, or from the
    /// caption buttons on the left.
    pub lead: f32,
    /// Each body's left edge, in order.
    pub bodies: Vec<f32>,
    /// The feet either side of a body, and the top corner radius.
    pub foot: f32,
    pub top_radius: f32,
    /// The highlight's corner radius for this body width.
    pub highlight_radius: f32,
    /// The favicon within a body (x from the body's left), and the title's
    /// left after it.
    pub favicon: Option<Rect>,
    pub title_left: f32,
    /// The title's width at most, and its top below the tab row's top so
    /// its baseline lies where Chrome's label puts it.
    pub title_width: f32,
    pub title_top: f32,
    pub title_baseline: f32,
    /// The close button within a body: its left from the body's left, its
    /// size (a circle).
    pub close: Option<Rect>,
    /// A separator between two bodies: its offset from the gap's middle,
    /// its size, its top below the strip's top.
    pub separator: Rect,
    /// The new-tab button after the last body.
    pub new_tab: Rect,
    /// The caption buttons placed.
    pub left_placed: Vec<PlacedButton>,
    pub right_placed: Vec<PlacedButton>,
}

/// DIP to whole pixels at `scale`, as Chrome's layout rounds.
pub fn dip(v: f32, scale: f32) -> f32 {
    (v * scale).round()
}

/// A tab body's width for `tabs` sharing `room` pixels: Chrome's standard
/// width when there is room, else shared out, never narrower than the
/// minimum.
pub fn body_width(room: f32, tabs: usize, scale: f32) -> f32 {
    let each = room / tabs.max(1) as f32 - dip(GAP, scale);
    each.clamp(dip(BODY_MIN, scale), dip(BODY_MAX, scale))
        .floor()
}

/// The highlight's corner radius for a tab `body_px` wide: the top radius,
/// or less for a narrow tab so a third of its top stays flat
/// (GetTopCornerRadiusForWidth, over the width with the feet).
pub fn highlight_radius(body_px: f32, scale: f32) -> f32 {
    let width = body_px / scale + 2. * FOOT;
    ((width - 2. * TOP_RADIUS) / 3.).clamp(0., TOP_RADIUS) * scale
}

/// Where the title's baseline lies below the strip's top, in pixels:
/// Chrome centres the cap height of a label spanning the whole 41-DIP tab
/// (RenderText::DetermineBaselineCenteringText) in whole pixels of the
/// font's rounded-up ascent, descent and cap height; without a cap height
/// the whole font height is centred, as Chrome does when its GetCapHeight
/// answers the ascent.
pub fn title_baseline(font: &FontMetrics, scale: f32) -> f32 {
    let a = font.ascent.ceil();
    let h = a + font.descent.ceil();
    let (k, leading) = match font.cap_height {
        Some(c) => (c.ceil(), a - c.ceil()),
        None => (a, 0.),
    };
    let display = dip(STRIP_HEIGHT, scale);
    let space = display - if leading != 0. { k } else { h };
    let shift = (space / 2.).trunc() - leading;
    let (min, max) = ((display - h).min(0.), (display - h).abs());
    a + shift.clamp(min, max)
}

/// A caption button picture placed as Chrome's header bar places it in
/// the 40-DIP top area (NavButtonProviderGtk::RedrawImages): the picture
/// with its CSS margins and the bar's padding centred, all of it scaled
/// down to fit when it would not.
pub fn place_button(image: &ButtonImage, scale: f32) -> PlacedButton {
    let top_area = VISIBLE;
    let needed = image.header_top
        + image.margin_top
        + image.height
        + image.margin_bottom
        + image.header_bottom;
    let factor = if needed > top_area {
        top_area / needed
    } else {
        1.
    };
    let available = top_area - factor * (image.header_top + image.header_bottom);
    let button = factor * (image.height + image.margin_top + image.margin_bottom);
    let offset = factor * (image.header_top + image.margin_top) + (available - button) / 2.;
    PlacedButton {
        factor,
        width: dip((factor * image.width).round(), scale),
        height: dip((factor * image.height).round(), scale),
        margin_left: dip((factor * image.margin_left).round(), scale),
        margin_right: dip((factor * image.margin_right).round(), scale),
        top: dip(offset.round(), scale),
    }
}

impl Layout {
    pub fn compute(inputs: &Inputs) -> Layout {
        let scale = inputs.scale;
        let d = |v: f32| dip(v, scale);
        let left_placed: Vec<PlacedButton> = inputs
            .left_buttons
            .iter()
            .map(|b| place_button(b, scale))
            .collect();
        let right_placed: Vec<PlacedButton> = inputs
            .right_buttons
            .iter()
            .map(|b| place_button(b, scale))
            .collect();
        let buttons_px: f32 = left_placed
            .iter()
            .chain(&right_placed)
            .map(|b| b.width + b.margin_left + b.margin_right)
            .sum::<f32>()
            .max(d(inputs.drawn_buttons));
        // the room for the bodies: less the buttons, the first body's lead
        // and feet, the last body's feet and the new-tab button
        let lead = match inputs.left_placed_last_margin(&left_placed) {
            Some(margin) => d(LEADING).max(margin),
            None => d(LEADING),
        };
        let room =
            inputs.width_px - buttons_px - lead - d(FOOT) - d(NEW_TAB_MARGIN) - d(BUTTON) - d(GAP);
        let body_width = body_width(room.max(0.), inputs.tabs, scale);
        let left_edge: f32 = left_placed
            .iter()
            .map(|b| b.width + b.margin_left + b.margin_right)
            .sum();
        let bodies = (0..inputs.tabs)
            .map(|i| left_edge + lead + i as f32 * (body_width + d(GAP)))
            .collect::<Vec<_>>();
        let tab_top = d(TAB_TOP);
        let tab_height = d(HIGHLIGHT_HEIGHT);
        let favicon = inputs.favicons.then(|| {
            Rect::new(
                d(INSET),
                tab_top + ((tab_height - d(FAVICON)) / 2.).round(),
                d(FAVICON),
                d(FAVICON),
            )
        });
        let title_left = match favicon {
            Some(f) => f.right() + d(FAVICON_GAP),
            None => d(INSET),
        };
        let close = inputs.close_buttons.then(|| {
            Rect::new(
                body_width - d(INSET / 2.) - d(BUTTON),
                tab_top,
                d(BUTTON),
                d(BUTTON),
            )
        });
        let title_right = match close {
            Some(c) => c.x,
            None => body_width - d(INSET),
        };
        let title_baseline = title_baseline(&inputs.font, scale);
        let title_top = (title_baseline - tab_top - inputs.font.baseline_in_cell)
            .round()
            .clamp(0., (tab_height - inputs.font.cell_height).max(0.));
        let separator = Rect::new(
            -(d(SEPARATOR.0) / 2.).round(),
            tab_top + ((tab_height - d(SEPARATOR.1)) / 2.).round(),
            d(SEPARATOR.0),
            d(SEPARATOR.1),
        );
        let last_right = bodies.last().map_or(left_edge + lead, |x| x + body_width);
        let new_tab = Rect::new(
            last_right + d(GAP / 2.) + d(NEW_TAB_MARGIN),
            tab_top,
            d(BUTTON),
            d(BUTTON),
        );
        Layout {
            scale,
            height: d(VISIBLE),
            tab_top,
            tab_height,
            tab_bottom_gap: d(VISIBLE) - tab_top - tab_height,
            body_width,
            lead,
            bodies,
            foot: d(FOOT),
            top_radius: d(TOP_RADIUS),
            highlight_radius: highlight_radius(body_width, scale),
            favicon,
            title_left,
            title_width: (title_right - title_left).max(0.),
            title_top,
            title_baseline,
            close,
            separator,
            new_tab,
            left_placed,
            right_placed,
        }
    }

    /// The highlight (a hovered inactive tab's fill) over the body at `x`.
    pub fn highlight(&self, body_x: f32) -> Rect {
        Rect::new(body_x, self.tab_top, self.body_width, self.tab_height)
    }

    /// The active tab's shape over the body at `x`: the body with its
    /// feet, from the tab's top to the content.
    pub fn shape(&self, body_x: f32) -> Rect {
        Rect::new(
            body_x - self.foot,
            self.tab_top,
            self.body_width + 2. * self.foot,
            self.height - self.tab_top,
        )
    }

    /// The separator between the bodies at `a` and `b` (a's left, b's left).
    pub fn separator_between(&self, a: f32, b: f32) -> Rect {
        let middle = ((a + self.body_width + b) / 2.).round();
        Rect::new(
            middle + self.separator.x,
            self.separator.y,
            self.separator.w,
            self.separator.h,
        )
    }
}

impl Inputs {
    /// The last caption button's margin on the left, when there are any.
    fn left_placed_last_margin(&self, placed: &[PlacedButton]) -> Option<f32> {
        placed.last().map(|b| b.margin_right)
    }
}

// ---------------------------------------------------------------- colours

/// The contrast below which the active tab gets its stroke, and the
/// contrast the stroke keeps from it (BrowserView::ShouldDrawTabStrokes,
/// kColorToolbarTopSeparator).
pub const STROKE_BELOW: f32 = 1.3;
pub const STROKE_CONTRAST: f32 = 2.0;
/// An inactive tab under the pointer without a theme's own colour: this
/// much of the active colour over the strip (tab_strip_color_mixer.cc).
pub const HOVER_BLEND: f32 = 0.4;

fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance (color_utils::GetRelativeLuminance).
pub fn luminance(c: SrgbaTuple) -> f32 {
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// WCAG's contrast ratio (color_utils::GetContrastRatio).
pub fn contrast(a: SrgbaTuple, b: SrgbaTuple) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `a` over `b` at `alpha` in sRGB (color_utils::AlphaBlend).
pub fn blend(a: SrgbaTuple, b: SrgbaTuple, alpha: f32) -> SrgbaTuple {
    let m = |x: f32, y: f32| x * alpha + y * (1. - alpha);
    SrgbaTuple(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2), 1.)
}

/// `fg` moved towards `target` only as far as it needs to reach `ratio`
/// against `bg` (color_utils::BlendForMinContrast); `None` if even
/// `target` does not.
pub fn min_contrast(
    fg: SrgbaTuple,
    bg: SrgbaTuple,
    target: SrgbaTuple,
    ratio: f32,
) -> Option<SrgbaTuple> {
    if contrast(fg, bg) >= ratio {
        return Some(fg);
    }
    if contrast(target, bg) < ratio {
        return None;
    }
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..12 {
        let mid = (lo + hi) / 2.;
        if contrast(blend(target, fg, mid), bg) >= ratio {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(blend(target, fg, hi))
}

/// Google Grey 900 and white, the stroke's targets.
pub const GREY_900: SrgbaTuple = SrgbaTuple(
    0x20 as f32 / 255.,
    0x21 as f32 / 255.,
    0x24 as f32 / 255.,
    1.,
);
pub const WHITE: SrgbaTuple = SrgbaTuple(1., 1., 1., 1.);

/// The active tab's stroke where it would not stand out from the strip:
/// its colour taken towards Grey 900 until it contrasts 2:1 with itself,
/// else towards white.
pub fn stroke(active: SrgbaTuple, frame: SrgbaTuple) -> Option<SrgbaTuple> {
    if contrast(active, frame) >= STROKE_BELOW {
        return None;
    }
    min_contrast(active, active, GREY_900, STROKE_CONTRAST)
        .or_else(|| min_contrast(active, active, WHITE, STROKE_CONTRAST))
}

/// An inactive tab under the pointer without a theme's own colour.
pub fn hover(active: SrgbaTuple, frame: SrgbaTuple) -> SrgbaTuple {
    blend(active, frame, HOVER_BLEND)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> SrgbaTuple {
        SrgbaTuple(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.)
    }

    fn lingmo_button() -> ButtonImage {
        ButtonImage {
            width: 36.,
            height: 36.,
            ..ButtonImage::default()
        }
    }

    #[test]
    fn the_strips_rows_at_scale_1() {
        let l = Layout::compute(&Inputs {
            tabs: 4,
            width_px: 1100.,
            ..Inputs::default()
        });
        assert_eq!(l.height, 40., "40 of the 41 show; the content at row 40");
        assert_eq!(
            (l.tab_top, l.tab_height, l.tab_bottom_gap),
            (6., 28., 6.),
            "the pill 6..34, centred"
        );
        assert_eq!(l.body_width, 232., "room to spare: the standard width");
        assert_eq!(l.lead, 12., "the first body 12 from the client edge");
        assert_eq!(l.bodies, vec![12., 250., 488., 726.], "bodies 6 apart");
        assert_eq!(
            l.favicon,
            Some(Rect::new(8., 12., 16., 16.)),
            "favicon rows 12..28, 8 in"
        );
        assert_eq!(l.title_left, 32., "the title 8 after the favicon");
        assert_eq!(
            l.close,
            Some(Rect::new(200., 6., 28., 28.)),
            "the close circle 4 from the right, rows 6..34"
        );
        assert_eq!(l.title_width, 168.);
        assert_eq!(
            l.separator,
            Rect::new(-1., 12., 2., 16.),
            "separators rows 12..28"
        );
        assert_eq!(
            l.separator_between(12., 250.),
            Rect::new(246., 12., 2., 16.)
        );
        assert_eq!(
            l.new_tab,
            Rect::new(963., 6., 28., 28.),
            "the new-tab button on the tab row"
        );
        assert_eq!(l.highlight(250.), Rect::new(250., 6., 232., 28.));
        assert_eq!(
            l.shape(250.),
            Rect::new(238., 6., 256., 34.),
            "the feet either side, down to the content"
        );
        assert_eq!((l.top_radius, l.foot, l.highlight_radius), (10., 12., 10.));
    }

    #[test]
    fn the_title_where_chromes_label_puts_it() {
        let l = Layout::compute(&Inputs::default());
        // Noto Sans CJK SC 15 px: ascent 18, cap 11 -> baseline row 26
        assert_eq!(l.title_baseline, 26.);
        assert_eq!(
            l.title_top, 3.,
            "the 22-px cell 3 below the tab row: baseline 6 + 3 + 17"
        );
        assert_eq!(
            title_baseline(
                &FontMetrics {
                    cap_height: None,
                    ..Inputs::default().font
                },
                1.
            ),
            27.,
            "no cap height: the font height centred"
        );
        assert_eq!(
            title_baseline(
                &FontMetrics {
                    ascent: 40.,
                    descent: 10.,
                    cap_height: Some(30.),
                    ..Inputs::default().font
                },
                1.
            ),
            35.,
            "a shift of 5 - 10, within the -9..9 the tab allows"
        );
        // a tall font's cell cannot leave the tab row
        let tall = Layout::compute(&Inputs {
            font: FontMetrics {
                ascent: 24.,
                descent: 8.,
                cap_height: Some(16.),
                cell_height: 32.,
                baseline_in_cell: 24.,
            },
            ..Inputs::default()
        });
        assert_eq!(tall.title_top, 0.);
    }

    #[test]
    fn scales() {
        for (scale, height, top, pill, favicon, close) in [
            (1.25, 50., 8., 35., Rect::new(10., 16., 20., 20.), 35.),
            (1.5, 60., 9., 42., Rect::new(12., 18., 24., 24.), 42.),
            (2., 80., 12., 56., Rect::new(16., 24., 32., 32.), 56.),
        ] {
            let l = Layout::compute(&Inputs {
                scale,
                ..Inputs::default()
            });
            assert_eq!(
                (l.height, l.tab_top, l.tab_height),
                (height, top, pill),
                "at {scale}"
            );
            assert_eq!(l.favicon, Some(favicon), "at {scale}");
            assert_eq!(l.close.unwrap().w, close, "the button round(28 * {scale})");
            assert_eq!(l.body_width, dip(BODY_MAX, scale));
        }
    }

    #[test]
    fn widths_shared_out() {
        let l = Layout::compute(&Inputs {
            tabs: 10,
            width_px: 800.,
            ..Inputs::default()
        });
        // 800 - 12 - 12 - 2 - 28 - 6 = 740 for 10: 74 - 6 = 68 each
        assert_eq!(l.body_width, 68.);
        assert_eq!(l.bodies[9], 12. + 9. * 74.);
        assert_eq!(
            l.highlight_radius, 10.,
            "a third of a 92-wide top is over 10 still"
        );
        let l = Layout::compute(&Inputs {
            tabs: 40,
            width_px: 800.,
            ..Inputs::default()
        });
        assert_eq!(l.body_width, 32., "never narrower than the minimum");
        assert_eq!(
            l.highlight_radius, 10.,
            "Chrome's narrowest tab keeps its radius"
        );
        assert_eq!(
            highlight_radius(10., 1.),
            14. / 3.,
            "only under 50 wide do the corners shrink"
        );
    }

    #[test]
    fn caption_buttons_placed_as_the_header_bar_places_them() {
        // Lingmo's 36-px pictures with no margins: centred in the 40
        let p = place_button(&lingmo_button(), 1.);
        assert_eq!((p.factor, p.width, p.height, p.top), (1., 36., 36., 2.));
        // a 44-px picture with 2 of header padding each side: shrunk to fit
        let big = ButtonImage {
            width: 44.,
            height: 44.,
            header_top: 2.,
            header_bottom: 2.,
            ..ButtonImage::default()
        };
        let p = place_button(&big, 1.);
        assert!((p.factor - 40. / 48.).abs() < 1e-6);
        assert_eq!((p.width, p.height), (37., 37.));
        assert_eq!(p.top, 2., "2 of padding scaled, then centred");
        // margins and padding go into the centring
        let m = ButtonImage {
            width: 24.,
            height: 24.,
            margin_top: 4.,
            header_top: 6.,
            header_bottom: 6.,
            ..ButtonImage::default()
        };
        let p = place_button(&m, 1.);
        assert_eq!(
            p.top, 10.,
            "6 of padding, 4 of margin, the 28 centred in 28"
        );
        // at 2x, whole pixels
        let p = place_button(&lingmo_button(), 2.);
        assert_eq!((p.width, p.top), (72., 4.));
    }

    #[test]
    fn the_first_tab_past_left_buttons() {
        let close = ButtonImage {
            width: 24.,
            height: 24.,
            margin_left: 6.,
            margin_right: 8.,
            ..ButtonImage::default()
        };
        let l = Layout::compute(&Inputs {
            left_buttons: vec![close],
            ..Inputs::default()
        });
        assert_eq!(l.lead, 12., "the larger of 12 and the button's margin");
        assert_eq!(l.bodies[0], 38. + 12.);
        let wide = ButtonImage {
            margin_right: 20.,
            ..close
        };
        let l = Layout::compute(&Inputs {
            left_buttons: vec![wide],
            ..Inputs::default()
        });
        assert_eq!(l.lead, 20.);
    }

    #[test]
    fn contrast_as_wcag() {
        assert!((contrast(rgb(0, 0, 0), rgb(255, 255, 255)) - 21.).abs() < 0.01);
        assert!((contrast(rgb(10, 10, 10), rgb(10, 10, 10)) - 1.).abs() < 0.001);
    }

    #[test]
    fn a_stroke_only_where_the_tab_would_not_show() {
        assert_eq!(stroke(rgb(0x0c, 0x0c, 0x0c), rgb(0x2e, 0x2e, 0x2e)), None);
        let s = stroke(rgb(0xfa, 0xfa, 0xfa), rgb(0xfa, 0xfa, 0xfa)).unwrap();
        assert!(contrast(s, rgb(0xfa, 0xfa, 0xfa)) >= 2.0 - 0.01);
        assert!(luminance(s) < luminance(rgb(0xfa, 0xfa, 0xfa)));
        let s = stroke(rgb(0, 0, 0), rgb(0, 0, 0)).unwrap();
        assert!(luminance(s) > 0.);
    }

    #[test]
    fn hover_without_a_theme_colour() {
        let h = hover(rgb(255, 255, 255), rgb(0, 0, 0));
        assert!((h.0 - 0.4).abs() < 1e-6);
    }
}
