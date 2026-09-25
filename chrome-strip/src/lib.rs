//! Chromium's tab strip as it draws it over a Linux desktop theme: every
//! size, position, colour rule and animation, computed here in pixels from
//! the inputs alone (the window's width, the scale, the tabs, the theme's
//! caption buttons, the title font's metrics), with nothing to draw them.
//!
//! The renderer takes its numbers from `Layout`; the tests here hold
//! them against Chromium's, as read out of its source in
//! `docs-internal/chrome-strip-spec.md` (layout_constants.cc,
//! tab_style.cc, tab_strip_layout.cc, horizontal_tab_style_views.cc,
//! tab.cc, nav_button_provider_gtk.cc, render_text.cc, color_utils.cc,
//! tab_strip_color_mixer.cc, native_chrome_color_mixer_linux.cc), so a
//! change is checked against Chrome on any machine, not by looking at a
//! desktop.
//!
//! Sizes are DIP (pixels at 96 dpi) unless named `_px`. Chrome lays the
//! strip out in whole DIP and turns them into pixels when it paints,
//! rounding as `ScaleAndAlignBounds` does; `Layout` gives both.

pub use wezterm_color_types::SrgbaTuple;

pub mod frame;

// ---------------------------------------------------------------- DIP

/// kTabStripHeight = kTabHeight (34 + kTabstripToolbarOverlap) +
/// kTabStripPadding. The last DIP lies under Chrome's toolbar: here the
/// terminal begins there, and the toolbar's top line is drawn on it.
pub const STRIP_HEIGHT: f32 = 41.;
pub const OVERLAP: f32 = 1.;
pub const TAB_TOP: f32 = 6.;
/// The highlight (a hovered inactive tab's fill) and the tab's contents:
/// kTabHeight less the padding and the overlap.
pub const HIGHLIGHT_HEIGHT: f32 = 28.;
/// TabStyle::GetTopCornerRadius, GetBottomCornerRadius (the feet).
pub const TOP_RADIUS: f32 = 10.;
pub const FOOT: f32 = 12.;
/// A tab's width with its feet: standard (kTabWidth 232 + 2 feet), the
/// least an active tab gets, the least an inactive one gets; tabs overlap
/// by two feet less the separator and its margins.
pub const TAB_STANDARD: f32 = 256.;
pub const TAB_MIN_ACTIVE: f32 = 56.;
pub const TAB_MIN_INACTIVE: f32 = 32.;
pub const TAB_OVERLAP: f32 = 18.;
/// Between two bodies (two feet less the overlap).
pub const GAP: f32 = 2. * FOOT - TAB_OVERLAP;
/// The contents' inset from the tab's edge (a foot + kTabHorizontalPadding
/// 8) and from the body's.
pub const CONTENT_INSET: f32 = 20.;
pub const INSET: f32 = CONTENT_INSET - FOOT;
/// TabStyle::GetSeparatorSize, its corner radius, its margins (6 above
/// and below, 2 either side) and where it stands from the tab's edge.
pub const SEPARATOR: (f32, f32) = (2., 16.);
pub const SEPARATOR_RADIUS: f32 = 1.;
pub const SEPARATOR_FROM_EDGE: f32 = FOOT - 2. - 2.;
/// gfx::kFaviconSize, kTabPreTitlePadding after it, kTabAfterTitlePadding
/// before what follows the title.
pub const FAVICON: f32 = 16.;
pub const FAVICON_GAP: f32 = 8.;
pub const AFTER_TITLE: f32 = 4.;
/// The close button: a 16-DIP icon in a 28-DIP button, whose hover
/// highlight is a circle of radius 8 around the icon at opacity 0.16 of
/// the colour that contrasts most with the tab.
pub const CLOSE_ICON: f32 = 16.;
pub const CLOSE_BUTTON: f32 = 28.;
pub const CLOSE_HIGHLIGHT_RADIUS: f32 = 8.;
pub const HIGHLIGHT_OPACITY: f32 = 0.16;
/// An inactive tab shows its close button only with this much contents
/// room (kMinimumContentsWidthForCloseButtons); a tab narrower than the
/// least inactive width shows nothing.
pub const CLOSE_ROOM: f32 = 68.;
/// The new-tab button: a 28-DIP circle with a 16-DIP icon, 6 DIP past
/// the last tab's right edge, on the tabs' row; the strip keeps 34 DIP
/// for it.
pub const NEW_TAB_BUTTON: f32 = 28.;
pub const NEW_TAB_ICON: f32 = 16.;
pub const NEW_TAB_AFTER_TABS: f32 = 6.;
pub const NEW_TAB_ROOM: f32 = NEW_TAB_BUTTON + NEW_TAB_AFTER_TABS;
/// The tab search button at the strip's leading end (TabStripComboButton's
/// kActionTabSearch button, a TabStripFlatEdgeButton): 28 square, 6 in
/// from the region's edge (AdjustViewBoundsRect: the strip's x + 12 - 6 -
/// 28, the strip's left margin being 28), its chevron icon 16
/// (kExpandMoreOldIcon: a 8.6 x 4.3 chevron at (3.7, 6.44) of the 16).
pub const TAB_SEARCH_BUTTON: f32 = 28.;
pub const TAB_SEARCH_INSET: f32 = 6.;
pub const TAB_SEARCH_CHEVRON: (f32, f32, f32, f32) = (3.7, 6.44, 8.6, 4.3);
/// The room kept free after the new-tab button, before the caption
/// buttons, for grabbing the frame by however full the strip is
/// (HorizontalTabStripRegionView's FrameGrabHandle: 42 x 0 preferred,
/// never narrower).
pub const GRAB_HANDLE: f32 = 42.;
/// The strip starts at the client edge, or past the leading caption
/// buttons by their margin over this much (the region's leading margin).
pub const LEADING_MARGIN: f32 = 12.;
/// The header bar's top area the caption buttons are centred in.
pub const TOP_AREA: f32 = STRIP_HEIGHT - OVERLAP;
/// The hover animation: 200 ms, eased out in, eased in out.
pub const HOVER_ANIMATION_MS: f32 = 200.;

/// A rectangle in pixels (or DIP, where said).
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
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// DIP to whole pixels at `scale`, as Chrome's layout rounds.
pub fn dip(v: f32, scale: f32) -> f32 {
    (v * scale).round()
}

// ---------------------------------------------------------------- inputs

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
    pub active: usize,
    /// The caption buttons on each side, in their order from the edge.
    pub left_buttons: Vec<ButtonImage>,
    pub right_buttons: Vec<ButtonImage>,
    /// The room drawn caption buttons take on the right when there are no
    /// pictures (DIP).
    pub drawn_buttons: f32,
    pub close_buttons: bool,
    pub favicons: bool,
    /// The tab search button before the tabs (Chrome's, on every normal
    /// window).
    pub tab_search: bool,
    pub font: FontMetrics,
}

impl Default for Inputs {
    fn default() -> Inputs {
        Inputs {
            scale: 1.,
            width_px: 1100.,
            tabs: 1,
            active: 0,
            left_buttons: vec![],
            right_buttons: vec![],
            drawn_buttons: 0.,
            close_buttons: true,
            favicons: true,
            tab_search: false,
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

// ---------------------------------------------------------------- widths

/// Chrome's tab widths (with their feet) for `tabs` tabs sharing
/// `available` DIP (tab_strip_layout.cc CalculateTabBounds): every tab
/// at its standard width when they fit; else all the same, between the
/// 56 both kinds cross at and 256; below that the active tab keeps 56
/// and the others shrink to 32. Widths are floored and the DIP left
/// over given one each to the first tabs.
pub fn tab_widths(available: f32, tabs: usize, active: usize) -> Vec<f32> {
    if tabs == 0 {
        return vec![];
    }
    let (min, cross, pref): (Vec<f32>, Vec<f32>, Vec<f32>) = (0..tabs)
        .map(|i| {
            if i == active {
                (TAB_MIN_ACTIVE, TAB_MIN_ACTIVE, TAB_STANDARD)
            } else {
                (TAB_MIN_INACTIVE, TAB_MIN_ACTIVE, TAB_STANDARD)
            }
        })
        .fold((vec![], vec![], vec![]), |mut acc, (a, b, c)| {
            acc.0.push(a);
            acc.1.push(b);
            acc.2.push(c);
            acc
        });
    let total = |w: &[f32]| w.iter().map(|w| w - TAB_OVERLAP).sum::<f32>() + TAB_OVERLAP;
    let (min_w, cross_w, pref_w) = (total(&min), total(&cross), total(&pref));
    let (f, lo, hi) = if available < cross_w {
        let f = if min_w == cross_w {
            1.
        } else {
            (available - min_w) / (cross_w - min_w)
        };
        (f, min, cross)
    } else {
        let f = if pref_w == cross_w {
            1.
        } else {
            (available - cross_w) / (pref_w - cross_w)
        };
        (f, cross, pref)
    };
    let f = f.clamp(0., 1.);
    let mut widths: Vec<f32> = (0..tabs)
        .map(|i| (lo[i] + (hi[i] - lo[i]) * f).floor())
        .collect();
    let at_preferred = available >= cross_w && f == 1.;
    let used = total(&widths);
    let mut extra = available - used;
    if !at_preferred && extra > 0. {
        for (i, w) in widths.iter_mut().enumerate() {
            if extra < 1. {
                break;
            }
            if f != 0. && f != 1. && lo[i] < hi[i] {
                *w += 1.;
                extra -= 1.;
            }
        }
    }
    widths
}

/// The top corners' radius for a tab `width` DIP wide (with its feet):
/// the ideal 10, or less so a third of the top stays flat
/// (GetTopCornerRadiusForWidth).
pub fn top_radius_for_width(width: f32) -> f32 {
    let top_width = (width - 2. * TOP_RADIUS).floor();
    (top_width / 3.).clamp(0., TOP_RADIUS)
}

// ---------------------------------------------------------------- text

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

// ---------------------------------------------------------------- caption buttons

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

/// A caption button picture placed as Chrome's header bar places it in
/// the 40-DIP top area (NavButtonProviderGtk::RedrawImages): the picture
/// with its CSS margins and the bar's padding centred, all of it scaled
/// down to fit when it would not.
pub fn place_button(image: &ButtonImage, scale: f32) -> PlacedButton {
    let needed = image.header_top
        + image.margin_top
        + image.height
        + image.margin_bottom
        + image.header_bottom;
    let factor = if needed > TOP_AREA {
        TOP_AREA / needed
    } else {
        1.
    };
    let available = TOP_AREA - factor * (image.header_top + image.header_bottom);
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

// ---------------------------------------------------------------- layout

/// One tab laid out, in pixels from the strip's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// The tab with its feet (Chrome's bounds), DIP.
    pub bounds_dip: Rect,
    /// The body between the feet, pixels, as `ScaleAndAlignBounds` aligns it.
    pub body: Rect,
    /// The active fill: the body with its feet, from the tab's top to the
    /// strip's bottom (the toolbar's first row included).
    pub shape: Rect,
    /// The highlight (a hovered inactive tab's fill), and its corner
    /// radius.
    pub highlight: Rect,
    pub radius: f32,
    /// Whether the tab shows at all: Chrome hides a tab clipped by the
    /// strip's trailing edge, and one before the active tab that would
    /// be clipped were it activated (TabContainerImpl::ShouldTabBeVisible);
    /// the tabs never shrink below their minimum widths to fit.
    pub visible: bool,
    /// The favicon, when shown: at the contents' left, or centred in a
    /// tab too narrow for anything else (tab.cc center_icon_).
    pub favicon: Option<Rect>,
    /// The title's left edge and width, and its top so the baseline lies
    /// where Chrome's label puts it.
    pub title_left: f32,
    pub title_width: f32,
    pub title_top: f32,
    /// The close button (28 square) when shown, and the icon within it.
    pub close: Option<Rect>,
    pub close_icon: Option<Rect>,
    /// The separators either side of the tab (Chrome's leading and
    /// trailing), shown by `separator_opacity`.
    pub leading_separator: Rect,
    pub trailing_separator: Rect,
}

/// The strip in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    /// The strip (41 DIP); the terminal begins at `height`, its first row
    /// being the toolbar's under Chrome, where the top line is drawn.
    pub height: f32,
    pub top_line: f32,
    /// A tab's row, below the strip's top.
    pub tab_top: f32,
    pub tab_height: f32,
    pub tabs: Vec<Tab>,
    pub title_baseline: f32,
    /// The new-tab button (a 28 circle) and its icon.
    pub new_tab: Rect,
    pub new_tab_icon: Rect,
    /// The tab search button and its chevron (the icon's own box), when
    /// asked for.
    pub tab_search: Option<Rect>,
    pub tab_search_chevron: Option<Rect>,
    /// The caption buttons placed.
    pub left_placed: Vec<PlacedButton>,
    pub right_placed: Vec<PlacedButton>,
}

/// Chrome's `Center` for a tab's contents: the extra room halved, an odd
/// pixel going to the leading side.
fn center(size: f32, item: f32) -> f32 {
    let mut extra = size - item;
    if extra > 0. {
        extra += 1.;
    }
    (extra / 2.).trunc()
}

impl Layout {
    pub fn compute(inputs: &Inputs) -> Layout {
        let scale = inputs.scale;
        let d = |v: f32| dip(v, scale);
        let width = inputs.width_px / scale;
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
        // the strip's region: from the client edge (or past the leading
        // buttons by their margin over 12) to the trailing buttons less
        // their margin
        let extent = |b: &PlacedButton| (b.width + b.margin_left + b.margin_right) / scale;
        let leading: f32 = left_placed.iter().map(extent).sum();
        let leading_margin = left_placed.last().map_or(0., |b| b.margin_right / scale);
        let region_left = if left_placed.is_empty() {
            0.
        } else {
            leading - leading_margin + leading_margin.max(LEADING_MARGIN)
        };
        // the tab search button 6 into the region, the tabs' strip 28
        // past its start (Chrome's strip margin for it)
        let (tab_search, tab_search_chevron) = if inputs.tab_search {
            let b = Rect::new(
                d(region_left + TAB_SEARCH_INSET),
                d(TAB_TOP),
                d(TAB_SEARCH_BUTTON),
                d(TAB_SEARCH_BUTTON),
            );
            let icon = (TAB_SEARCH_BUTTON - NEW_TAB_ICON) / 2.;
            let (cx, cy, cw, ch) = TAB_SEARCH_CHEVRON;
            let c = Rect::new(
                d(region_left + TAB_SEARCH_INSET + icon + cx),
                d(TAB_TOP + icon + cy),
                d(cw),
                d(ch),
            );
            (Some(b), Some(c))
        } else {
            (None, None)
        };
        let region_left = if inputs.tab_search {
            region_left + TAB_SEARCH_BUTTON
        } else {
            region_left
        };
        // (the first trailing button's own left margin lies outside the
        // buttons' bounds; the caption margin, its right margin, is added)
        let trailing: f32 = right_placed
            .first()
            .map(|first| {
                right_placed.iter().map(extent).sum::<f32>()
                    - (first.margin_left - first.margin_right) / scale
            })
            .unwrap_or(0.)
            .max(inputs.drawn_buttons);
        let region_right = width - trailing;
        // the tabs share the region less the new-tab button's room and
        // the grab handle's
        let available = (region_right - region_left - NEW_TAB_ROOM - GRAB_HANDLE).max(0.);
        let widths = tab_widths(available, inputs.tabs, inputs.active);
        let tab_top = d(TAB_TOP);
        let tab_height = d(HIGHLIGHT_HEIGHT);
        let title_baseline = title_baseline(&inputs.font, scale);
        let title_top = (title_baseline - tab_top - inputs.font.baseline_in_cell)
            .round()
            .clamp(0., (tab_height - inputs.font.cell_height).max(0.));
        let mut x = region_left;
        let mut tabs = Vec::with_capacity(widths.len());
        for (i, &w) in widths.iter().enumerate() {
            let active = i == inputs.active;
            let bounds_dip = Rect::new(x, 0., w, STRIP_HEIGHT);
            // the body's edges where Chrome aligns them to pixels
            let body_left = ((x + FOOT) * scale).round();
            let body_right = ((x + w - FOOT - 2.) * scale).round() + 2. * scale;
            let body = Rect::new(body_left, tab_top, body_right - body_left, tab_height);
            let radius = top_radius_for_width(w) * scale;
            // what the tab shows (tab.cc UpdateIconVisibility): an inactive
            // tab with room for nothing else still shows its favicon,
            // centred in the tab
            let (favicon_shown, close_shown, favicon_centred) = if w < TAB_MIN_INACTIVE {
                (false, false, false)
            } else {
                let mut avail = w - 2. * CONTENT_INSET;
                if active {
                    let close = inputs.close_buttons;
                    if close {
                        avail -= CLOSE_ICON + AFTER_TITLE;
                    }
                    (inputs.favicons && avail >= FAVICON, close, false)
                } else {
                    let favicon = inputs.favicons && avail >= FAVICON;
                    let close = inputs.close_buttons && avail >= CLOSE_ROOM;
                    if !favicon && !close && inputs.favicons {
                        (true, false, true)
                    } else {
                        (favicon, close, false)
                    }
                }
            };
            let contents_left = x + CONTENT_INSET;
            let contents_right = x + w - CONTENT_INSET;
            let favicon = favicon_shown.then(|| {
                let left = if favicon_centred {
                    x + center(w, FAVICON)
                } else {
                    contents_left
                };
                Rect::new(
                    d(left),
                    d(STRIP_HEIGHT - HIGHLIGHT_HEIGHT - 1.),
                    d(FAVICON),
                    d(FAVICON),
                )
            });
            let (close, close_icon) = if close_shown {
                let visible_left = (contents_right - CLOSE_ICON).max(x + center(w, CLOSE_ICON));
                let pad = (CLOSE_BUTTON - CLOSE_ICON) / 2.;
                (
                    Some(Rect::new(
                        d(visible_left - pad),
                        tab_top,
                        d(CLOSE_BUTTON),
                        d(CLOSE_BUTTON),
                    )),
                    Some(Rect::new(
                        d(visible_left),
                        tab_top + d(pad),
                        d(CLOSE_ICON),
                        d(CLOSE_ICON),
                    )),
                )
            } else {
                (None, None)
            };
            let title_left = if favicon_centred {
                x + center(w, FAVICON) + FAVICON + FAVICON_GAP
            } else if favicon_shown {
                contents_left + FAVICON + FAVICON_GAP
            } else {
                contents_left
            };
            let title_right = if close_shown {
                contents_right - CLOSE_ICON - 2. * AFTER_TITLE
            } else {
                contents_right
            };
            let separator_y = tab_top + ((tab_height - d(SEPARATOR.1)) / 2.).trunc();
            let separator = |sx: f32| Rect::new(d(sx), separator_y, d(SEPARATOR.0), d(SEPARATOR.1));
            tabs.push(Tab {
                bounds_dip,
                visible: true,
                body,
                shape: Rect::new(
                    body_left - d(FOOT),
                    tab_top,
                    body.w + 2. * d(FOOT),
                    d(STRIP_HEIGHT) - tab_top,
                ),
                highlight: Rect::new(
                    body_left,
                    tab_top,
                    body.w,
                    (tab_top + HIGHLIGHT_HEIGHT * scale).trunc() - tab_top,
                ),
                radius,
                favicon,
                title_left: d(title_left),
                title_width: (d(title_right) - d(title_left)).max(0.),
                title_top,
                close,
                close_icon,
                leading_separator: separator(x + SEPARATOR_FROM_EDGE),
                trailing_separator: separator(x + w - SEPARATOR_FROM_EDGE - SEPARATOR.0),
            });
            x += w - TAB_OVERLAP;
        }
        // the tabs the strip has room for (TabContainerImpl::ShouldTabBeVisible):
        // one past the strip's trailing edge is hidden whole, as is one
        // before the active tab that would be past it were it the active
        // one (wider by the difference)
        let strip_right = region_left + available;
        let active_w = widths.get(inputs.active).copied().unwrap_or(0.);
        for (i, t) in tabs.iter_mut().enumerate() {
            let right = t.bounds_dip.right();
            t.visible = right <= strip_right
                && (inputs.active <= i || right + active_w - t.bounds_dip.w <= strip_right);
        }
        // the new-tab button after the strip: its bounds end where the
        // tabs do, or at its allotted width when they run past it
        let tabs_right = tabs
            .last()
            .map_or(region_left, |t| t.bounds_dip.right())
            .min(strip_right);
        let new_tab = Rect::new(
            d(tabs_right - FOOT + NEW_TAB_AFTER_TABS),
            tab_top,
            d(NEW_TAB_BUTTON),
            d(NEW_TAB_BUTTON),
        );
        let icon_inset = d((NEW_TAB_BUTTON - NEW_TAB_ICON) / 2.);
        Layout {
            scale,
            height: d(STRIP_HEIGHT),
            top_line: d(STRIP_HEIGHT - OVERLAP),
            tab_top,
            tab_height,
            tabs,
            title_baseline,
            new_tab_icon: Rect::new(
                new_tab.x + icon_inset,
                new_tab.y + icon_inset,
                d(NEW_TAB_ICON),
                d(NEW_TAB_ICON),
            ),
            new_tab,
            tab_search,
            tab_search_chevron,
            left_placed,
            right_placed,
        }
    }

    /// Which tab the pointer at (`x`, `y`) is on: Chrome's hit test is the
    /// tab's whole row below the strip's padding (to the top when the
    /// frame is condensed), the body plus 3 DIP into each separator.
    pub fn tab_at(&self, x: f32, y: f32, condensed: bool) -> Option<usize> {
        let top = if condensed { 0. } else { self.tab_top };
        if y < top || y >= self.height {
            return None;
        }
        let reach = dip(3., self.scale);
        self.tabs
            .iter()
            .position(|t| t.visible && x >= t.body.x - reach && x < t.body.right() + reach)
    }
}

/// How much of the separator between tabs `a` and `b` (`b` right after
/// `a`) shows: none beside a tab that is active, else 1 less the larger
/// of the two hovers, so it fades with a neighbour's hover
/// (GetSeparatorOpacity).
pub fn separator_opacity(a_active: bool, a_hover: f32, b_active: bool, b_hover: f32) -> f32 {
    if a_active || b_active {
        return 0.;
    }
    (1. - a_hover.max(b_hover)).clamp(0., 1.)
}

// ---------------------------------------------------------------- hover card

/// Chrome's tab hover card (tab_hover_card_bubble_view.cc): as wide as a
/// standard tab, its corners 8, its text inset 12 (the title 4 above the
/// second line), a 256 x 144 preview below; it comes 300 ms after the
/// pointer rests on a narrow tab, 800 on a standard one and 500 more on
/// top of that, fades in over 200 ms and out over 150.
pub const HOVER_CARD_WIDTH: f32 = 256.;
pub const HOVER_CARD_RADIUS: f32 = 8.;
pub const HOVER_CARD_MARGIN: f32 = 12.;
pub const HOVER_CARD_LINE_GAP: f32 = 4.;
pub const HOVER_CARD_PREVIEW: (f32, f32) = (256., 144.);
pub const HOVER_CARD_FADE_IN_MS: f32 = 200.;
pub const HOVER_CARD_FADE_OUT_MS: f32 = 150.;
/// The card's shadow: two layers, both 2 DIP down, blurred 8 and 12
/// (Skia's blur, twice the CSS blur), at 20% and 10% of black.
pub const HOVER_CARD_SHADOWS: [(f32, f32, f32); 2] = [(2., 8., 0.20), (2., 12., 0.10)];
const HOVER_CARD_DELAY_MIN_MS: f32 = 300.;
const HOVER_CARD_DELAY_MAX_MS: f32 = 800.;
const HOVER_CARD_DELAY_EXTRA_MS: f32 = 500.;
const HOVER_CARD_NARROW_TAB: f32 = 64.;

/// How long the pointer rests before the card shows, by the widest tab
/// in the strip (DIP, with its feet): 300 ms up to a pinned tab's 64,
/// rising to 800 at the standard 256, where 500 ms more are added.
pub fn hover_card_delay_ms(widest_tab: f32) -> f32 {
    if widest_tab <= HOVER_CARD_NARROW_TAB {
        return HOVER_CARD_DELAY_MIN_MS;
    }
    let t = ((widest_tab - HOVER_CARD_NARROW_TAB) / (TAB_STANDARD - HOVER_CARD_NARROW_TAB))
        .clamp(0., 1.);
    let delay = HOVER_CARD_DELAY_MIN_MS + (HOVER_CARD_DELAY_MAX_MS - HOVER_CARD_DELAY_MIN_MS) * t;
    if widest_tab >= TAB_STANDARD {
        delay + HOVER_CARD_DELAY_EXTRA_MS
    } else {
        delay
    }
}

// ---------------------------------------------------------------- animation

/// Chrome's hover animation: 200 ms, eased out coming in (1 - (1 - t)^2),
/// eased in going out (t^2); reversed midway it goes on from where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hover {
    from: f32,
    to: f32,
    start_ms: f64,
}

impl Hover {
    pub fn at(value: f32) -> Hover {
        Hover {
            from: value,
            to: value,
            start_ms: 0.,
        }
    }

    /// The value at `now_ms`.
    pub fn value(&self, now_ms: f64) -> f32 {
        if self.from == self.to {
            return self.to;
        }
        let t = ((now_ms - self.start_ms) / f64::from(HOVER_ANIMATION_MS)).clamp(0., 1.) as f32;
        let eased = if self.to > self.from {
            1. - (1. - t) * (1. - t)
        } else {
            t * t
        };
        self.from + (self.to - self.from) * eased
    }

    pub fn done(&self, now_ms: f64) -> bool {
        self.from == self.to || now_ms - self.start_ms >= f64::from(HOVER_ANIMATION_MS)
    }

    /// Start towards `target` (1 hovered, 0 not) at `now_ms`, from
    /// wherever the animation is.
    pub fn towards(&self, target: f32, now_ms: f64) -> Hover {
        if self.to == target {
            return *self;
        }
        Hover {
            from: self.value(now_ms),
            to: target,
            start_ms: now_ms,
        }
    }
}

// ---------------------------------------------------------------- colours

/// The contrast below which the active tab gets its stroke
/// (BrowserView::ShouldDrawTabStrokes), and the contrast the stroke keeps
/// from it (kColorToolbarTopSeparator).
pub const STROKE_BELOW: f32 = 1.3;
pub const STROKE_CONTRAST: f32 = 2.0;
/// color_utils: the luminance a colour is dark below, Google Grey 900,
/// white.
pub const DARK_BELOW: f32 = 0.211_692_04;
pub const GREY_900: SrgbaTuple = SrgbaTuple(
    0x20 as f32 / 255.,
    0x21 as f32 / 255.,
    0x24 as f32 / 255.,
    1.,
);
pub const WHITE: SrgbaTuple = SrgbaTuple(1., 1., 1., 1.);
/// Chrome's baseline palette, where the desktop has no accent: the hover
/// fill in light (primary 80) and dark (secondary 30).
pub const HOVER_LIGHT: SrgbaTuple = SrgbaTuple(
    0xA8 as f32 / 255.,
    0xC7 as f32 / 255.,
    0xFA as f32 / 255.,
    1.,
);
pub const HOVER_DARK: SrgbaTuple = SrgbaTuple(0., 0x4A as f32 / 255., 0x77 as f32 / 255., 1.);
/// Neutral 10 and 99: the hover over an unfocused window's tab is one of
/// them, translucent (kColorSysStateHoverOnSubtle: 0x0F of neutral 10 in
/// light, 0x1A of neutral 99 in dark).
pub const NEUTRAL_10: SrgbaTuple = SrgbaTuple(
    0x1F as f32 / 255.,
    0x1F as f32 / 255.,
    0x1F as f32 / 255.,
    1.,
);
pub const NEUTRAL_99: SrgbaTuple = SrgbaTuple(
    0xFD as f32 / 255.,
    0xFC as f32 / 255.,
    0xFB as f32 / 255.,
    1.,
);
pub const HOVER_UNFOCUSED_LIGHT_ALPHA: f32 = 0x0F as f32 / 255.;
pub const HOVER_UNFOCUSED_DARK_ALPHA: f32 = 0x1A as f32 / 255.;

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

/// color_utils::IsDark.
pub fn is_dark(c: SrgbaTuple) -> bool {
    luminance(c) < DARK_BELOW
}

/// color_utils::GetColorWithMaxContrast: white on a dark colour, Google
/// Grey 900 on a light one.
pub fn max_contrast(c: SrgbaTuple) -> SrgbaTuple {
    if is_dark(c) {
        WHITE
    } else {
        GREY_900
    }
}

/// WCAG's contrast ratio (color_utils::GetContrastRatio).
pub fn contrast(a: SrgbaTuple, b: SrgbaTuple) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `a` over `b` at `alpha` in sRGB (color_utils::AlphaBlend), opaque.
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

/// The active tab's stroke, and the toolbar's top line, where the tab
/// would not stand out from the strip: the tab's colour taken towards
/// Grey 900 until it contrasts 2:1 with itself, else towards white.
pub fn stroke(active: SrgbaTuple, frame: SrgbaTuple) -> Option<SrgbaTuple> {
    if contrast(active, frame) >= STROKE_BELOW {
        return None;
    }
    min_contrast(active, active, GREY_900, STROKE_CONTRAST)
        .or_else(|| min_contrast(active, active, WHITE, STROKE_CONTRAST))
}

/// The fill of an inactive tab under the pointer, fully hovered: in a
/// focused window the theme's own (the desktop accent's tone, which the
/// configuration gives) or Chrome's baseline for the strip's darkness;
/// in an unfocused window a translucent neutral over the strip.
pub fn hover_fill(theme: Option<SrgbaTuple>, frame: SrgbaTuple, focused: bool) -> SrgbaTuple {
    let dark = is_dark(frame);
    if focused {
        theme.unwrap_or(if dark { HOVER_DARK } else { HOVER_LIGHT })
    } else if dark {
        blend(NEUTRAL_99, frame, HOVER_UNFOCUSED_DARK_ALPHA)
    } else {
        blend(NEUTRAL_10, frame, HOVER_UNFOCUSED_LIGHT_ALPHA)
    }
}

/// The colour an inactive tab shows as its hover comes and goes
/// (PaintBackgroundHover): the hover fill over the strip by the
/// animation's value.
pub fn hover_at(fill: SrgbaTuple, frame: SrgbaTuple, value: f32) -> SrgbaTuple {
    blend(fill, frame, value)
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
    fn a_strip_of_four_at_scale_1() {
        let l = Layout::compute(&Inputs {
            tabs: 4,
            active: 1,
            width_px: 1200.,
            ..Inputs::default()
        });
        assert_eq!(
            (l.height, l.top_line),
            (41., 40.),
            "41 DIP, the toolbar's line on the last"
        );
        assert_eq!((l.tab_top, l.tab_height), (6., 28.));
        let t = &l.tabs[0];
        assert_eq!(
            t.bounds_dip,
            Rect::new(0., 0., 256., 41.),
            "Chrome's standard tab at the client edge"
        );
        assert_eq!(
            t.body,
            Rect::new(12., 6., 232., 28.),
            "the body 12 in, between the feet"
        );
        assert_eq!(
            t.shape,
            Rect::new(0., 6., 256., 35.),
            "the fill with its feet, down to the content"
        );
        assert_eq!(
            t.highlight,
            Rect::new(12., 6., 232., 28.),
            "the pill rows 6..34"
        );
        assert_eq!(t.radius, 10.);
        assert_eq!(
            t.favicon,
            Some(Rect::new(20., 12., 16., 16.)),
            "favicon at the contents' corner, rows 12..28"
        );
        assert_eq!(
            (t.title_left, t.title_width),
            (44., 168.),
            "8 after the favicon, 8 before the close icon"
        );
        assert_eq!(
            t.close,
            Some(Rect::new(214., 6., 28., 28.)),
            "the close button, its icon 16 from the contents' right"
        );
        assert_eq!(t.close_icon, Some(Rect::new(220., 12., 16., 16.)));
        assert_eq!(t.leading_separator, Rect::new(8., 12., 2., 16.));
        assert_eq!(t.trailing_separator, Rect::new(246., 12., 2., 16.));
        assert_eq!(l.tabs[1].bounds_dip.x, 238., "tabs overlap by 18");
        assert_eq!(l.tabs[1].body.x, 250., "bodies 6 apart");
        assert_eq!(
            l.tabs[1].leading_separator, l.tabs[0].trailing_separator,
            "one separator between two tabs"
        );
        assert_eq!(l.tabs[3].bounds_dip.right(), 3. * 238. + 256.);
        assert_eq!(
            l.new_tab,
            Rect::new(3. * 238. + 256. - 6., 6., 28., 28.),
            "the new-tab button 6 past the last tab"
        );
        assert_eq!(l.new_tab_icon, Rect::new(l.new_tab.x + 6., 12., 16., 16.));
    }

    #[test]
    fn the_title_where_chromes_label_puts_it() {
        let l = Layout::compute(&Inputs::default());
        // Noto Sans CJK SC 15 px: ascent 18, cap 11 -> baseline row 26
        assert_eq!(l.title_baseline, 26.);
        assert_eq!(
            l.tabs[0].title_top, 3.,
            "the 22-px cell 3 below the tab row: 6 + 3 + 17 = 26"
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
        assert_eq!(
            tall.tabs[0].title_top, 0.,
            "a tall cell cannot leave the tab row"
        );
    }

    #[test]
    fn scales() {
        for (scale, height, top, pill, favicon, close) in [
            (1.25, 51., 8., 35., Rect::new(25., 15., 20., 20.), 35.),
            (1.5, 62., 9., 42., Rect::new(30., 18., 24., 24.), 42.),
            (2., 82., 12., 56., Rect::new(40., 24., 32., 32.), 56.),
        ] {
            let l = Layout::compute(&Inputs {
                scale,
                width_px: 1100. * scale,
                ..Inputs::default()
            });
            assert_eq!(
                (l.height, l.tab_top, l.tab_height),
                (height, top, pill),
                "at {scale}"
            );
            assert_eq!(l.tabs[0].favicon, Some(favicon), "at {scale}");
            assert_eq!(
                l.tabs[0].close.unwrap().w,
                close,
                "the button round(28 * {scale})"
            );
            let t = &l.tabs[0];
            assert_eq!(
                (t.body.x, t.body.right()),
                ((12. * scale).round(), (242. * scale).round() + 2. * scale),
                "the body's edges as ScaleAndAlignBounds aligns them, at {scale}"
            );
            assert_eq!(
                l.tabs[0].highlight.h,
                (top + 28. * scale).trunc() - top,
                "int(top + 28s) as Chrome"
            );
        }
    }

    #[test]
    fn widths_as_chrome_shares_them() {
        // room to spare: every tab standard
        assert_eq!(tab_widths(1000., 3, 0), vec![256., 256., 256.]);
        // between the crossing point and the standard: all the same, the
        // DIP left over to the first tabs
        let w = tab_widths(400., 4, 0);
        // cross_w = 4*38+18 = 170, pref_w = 4*238+18 = 970; f = 230/800
        assert_eq!(w, vec![114., 114., 113., 113.]);
        assert_eq!(
            w.iter().map(|w| w - 18.).sum::<f32>() + 18.,
            400.,
            "they fill the room exactly"
        );
        // below it: the active tab keeps 56, the others shrink towards 32
        let w = tab_widths(100., 3, 1);
        // min_w = 14+38+14+18 = 84, cross_w = 3*38+18 = 132; f = 16/48
        assert_eq!(w[1], 56.);
        assert!(w[0] < 56. && w[0] >= 32., "{:?}", w);
        assert_eq!(w[0], w[2]);
        // nothing narrower than the least
        let w = tab_widths(10., 3, 0);
        assert_eq!(w, vec![56., 32., 32.]);
        assert_eq!(tab_widths(500., 0, 0), Vec::<f32>::new());
    }

    #[test]
    fn tabs_past_the_strip_are_hidden_whole() {
        // 60 tabs in a 600-px window (no caption buttons: the region runs
        // from 12 to the edge): at their minimum widths they run past the
        // strip; Chrome shows those that fit and hides the rest
        let l = Layout::compute(&Inputs {
            tabs: 60,
            width_px: 600.,
            active: 59,
            ..Inputs::default()
        });
        let strip_right = 600. - 34. - 42.;
        let shown: Vec<&Tab> = l.tabs.iter().filter(|t| t.visible).collect();
        assert!(shown.len() < 60 && shown.len() > 20, "{}", shown.len());
        assert!(shown.iter().all(|t| t.bounds_dip.right() <= strip_right));
        assert!(
            !l.tabs[59].visible,
            "the active tab past the edge is hidden too"
        );
        assert_eq!(
            l.tabs[0].bounds_dip.w, 32.,
            "never narrower than the minimum"
        );
        // the new-tab button sits 6 past the strip's own edge, not the tabs'
        assert_eq!(l.new_tab.x, strip_right - 12. + 6.);
        // a tab before the active one that would be clipped when activated
        // (24 wider) is hidden, though it fits as it is
        let last_fit = l
            .tabs
            .iter()
            .rposition(|t| t.bounds_dip.right() <= strip_right)
            .unwrap();
        let last_shown = l.tabs.iter().rposition(|t| t.visible).unwrap();
        assert!(last_shown <= last_fit);
        assert_eq!(
            l.tabs[last_fit].visible,
            l.tabs[last_fit].bounds_dip.right() + 56. - 32. <= strip_right,
        );
        assert_eq!(
            l.tab_at(l.tabs[59].body.x + 1., 20., false),
            None,
            "no hit on a hidden tab"
        );
        // a narrow inactive tab shows its favicon centred, nothing else
        let t = &l.tabs[1];
        assert_eq!(t.bounds_dip.w, 32.);
        assert_eq!(t.favicon.map(|f| f.x), Some(t.bounds_dip.x + 8.));
        assert_eq!(t.title_width, 0.);
        assert!(t.close.is_none());
    }

    #[test]
    fn the_tab_search_button_before_the_tabs() {
        let l = Layout::compute(&Inputs {
            tabs: 2,
            tab_search: true,
            ..Inputs::default()
        });
        // 6 into the region, 28 square, on the tabs' row
        assert_eq!(l.tab_search, Some(Rect::new(6., 6., 28., 28.)));
        // the tabs' strip 28 past the region's start: the first body at 40
        assert_eq!(l.tabs[0].bounds_dip.x, 28.);
        assert_eq!(l.tabs[0].body.x, 40.);
        // the chevron in the 16 icon centred in the button, on whole pixels
        assert_eq!(l.tab_search_chevron, Some(Rect::new(16., 18., 9., 4.)));
        // past leading caption buttons the same 6 in
        let l = Layout::compute(&Inputs {
            tabs: 2,
            tab_search: true,
            left_buttons: vec![ButtonImage {
                width: 24.,
                height: 24.,
                margin_left: 8.,
                margin_right: 2.,
                ..ButtonImage::default()
            }],
            ..Inputs::default()
        });
        assert_eq!(l.tab_search.map(|r| r.x), Some(34. + 12. - 2. + 6.));
        let none = Layout::compute(&Inputs {
            tabs: 2,
            ..Inputs::default()
        });
        assert_eq!(none.tab_search, None);
        assert_eq!(none.tabs[0].bounds_dip.x, 0.);
    }

    #[test]
    fn narrow_tabs_lose_their_close_buttons_and_corners() {
        // 40 tabs in 600 px: inactive tabs at 32, the active at 56
        let l = Layout::compute(&Inputs {
            tabs: 40,
            width_px: 600.,
            active: 3,
            ..Inputs::default()
        });
        assert_eq!(l.tabs[0].bounds_dip.w, 32.);
        assert_eq!(l.tabs[3].bounds_dip.w, 56.);
        assert!(
            l.tabs[0].close.is_none(),
            "an inactive tab needs 68 of room for its close button"
        );
        assert!(
            l.tabs[3].close.is_some(),
            "the active tab keeps its close button"
        );
        assert_eq!(
            l.tabs[0].favicon.map(|f| f.x),
            Some(8.),
            "32 wide: the favicon alone, centred"
        );
        assert_eq!(
            top_radius_for_width(32.),
            4.,
            "a third of the 12 left of the top"
        );
        assert_eq!(top_radius_for_width(50.), 10.);
        assert_eq!(top_radius_for_width(256.), 10.);
        // an inactive tab of 108 gets its close button back, 107 not
        let strip_for = |w: f32| Inputs {
            tabs: 8,
            width_px: w * 8. - 7. * 18. + 34. + 42.,
            active: 0,
            ..Inputs::default()
        };
        let l = Layout::compute(&strip_for(108.));
        assert_eq!(l.tabs[1].bounds_dip.w, 108.);
        assert!(l.tabs[1].close.is_some());
        let l = Layout::compute(&strip_for(107.));
        assert_eq!(l.tabs[1].bounds_dip.w, 107.);
        assert!(l.tabs[1].close.is_none());
    }

    #[test]
    fn caption_buttons_placed_as_the_header_bar_places_them() {
        let p = place_button(&lingmo_button(), 1.);
        assert_eq!((p.factor, p.width, p.height, p.top), (1., 36., 36., 2.));
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
        let m = ButtonImage {
            width: 24.,
            height: 24.,
            margin_top: 4.,
            header_top: 6.,
            header_bottom: 6.,
            ..ButtonImage::default()
        };
        assert_eq!(
            place_button(&m, 1.).top,
            10.,
            "6 of padding, 4 of margin, the 28 centred in 28"
        );
        let p = place_button(&lingmo_button(), 2.);
        assert_eq!((p.width, p.top), (72., 4.));
    }

    #[test]
    fn the_strip_past_caption_buttons() {
        let close = ButtonImage {
            width: 24.,
            height: 24.,
            margin_left: 6.,
            margin_right: 8.,
            ..ButtonImage::default()
        };
        // leading: the strip starts past the button by the larger of 12 and its margin
        let l = Layout::compute(&Inputs {
            left_buttons: vec![close],
            ..Inputs::default()
        });
        assert_eq!(l.tabs[0].bounds_dip.x, 30. + 12., "6 + 24 + max(12, 8)");
        assert_eq!(l.tabs[0].body.x, 54.);
        let wide = ButtonImage {
            margin_right: 20.,
            ..close
        };
        let l = Layout::compute(&Inputs {
            left_buttons: vec![wide],
            ..Inputs::default()
        });
        assert_eq!(l.tabs[0].bounds_dip.x, 30. + 20.);
        // trailing: the tabs share what is left before the buttons and the new-tab button
        let l = Layout::compute(&Inputs {
            tabs: 5,
            width_px: 1000.,
            right_buttons: vec![close; 3],
            ..Inputs::default()
        });
        // three 24-wide buttons with 6 + 8 of margin: their bounds run from
        // 1000 - (3 * 38 - 6) = 892, less the 8 caption margin
        let trailing = 3. * 38. - 6. + 8.;
        eprintln!(
            "{:?}",
            l.tabs.iter().map(|t| t.bounds_dip).collect::<Vec<_>>()
        );
        assert_eq!(
            l.tabs[4].bounds_dip.right() + 34. + 42.,
            1000. - trailing,
            "the new-tab button's 34 kept, and the grab handle's 42"
        );
        assert_eq!(
            l.new_tab.right(),
            1000. - trailing - 12. - 42.,
            "6 past the last tab, the grab handle short of the buttons' region"
        );
    }

    #[test]
    fn hit_testing_a_tab() {
        let l = Layout::compute(&Inputs {
            tabs: 2,
            ..Inputs::default()
        });
        assert_eq!(l.tab_at(100., 20., false), Some(0));
        assert_eq!(l.tab_at(100., 3., false), None, "above the tabs' padding");
        assert_eq!(
            l.tab_at(100., 3., true),
            Some(0),
            "a maximized window's tabs reach the top"
        );
        assert_eq!(l.tab_at(246., 20., false), Some(0), "3 into the separator");
        assert_eq!(l.tab_at(247., 20., false), Some(1));
        assert_eq!(l.tab_at(100., 41., false), None);
    }

    #[test]
    fn separators_beside_active_and_hovered_tabs() {
        assert_eq!(separator_opacity(false, 0., false, 0.), 1.);
        assert_eq!(separator_opacity(true, 0., false, 0.), 0.);
        assert_eq!(
            separator_opacity(false, 0., false, 0.25),
            0.75,
            "fading with the neighbour's hover"
        );
    }

    #[test]
    fn the_hover_cards_delay() {
        assert_eq!(hover_card_delay_ms(40.), 300.);
        assert_eq!(hover_card_delay_ms(64.), 300.);
        assert_eq!(hover_card_delay_ms(160.), 550.);
        assert_eq!(
            hover_card_delay_ms(256.),
            1300.,
            "800 and the 500 more at the standard width"
        );
        assert_eq!(hover_card_delay_ms(300.), 1300.);
    }

    #[test]
    fn the_hover_animation() {
        let h = Hover::at(0.).towards(1., 1000.);
        assert_eq!(h.value(1000.), 0.);
        assert!((h.value(1100.) - 0.75).abs() < 1e-6, "eased out: 1 - 0.5^2");
        assert_eq!(h.value(1200.), 1.);
        assert!(h.done(1200.));
        // reversed midway: from where it is, eased in
        let back = h.towards(0., 1100.);
        assert!((back.value(1100.) - 0.75).abs() < 1e-6);
        assert!(
            (back.value(1200.) - 0.75 * 0.75).abs() < 1e-6,
            "0.75 * (1 - 0.5^2)"
        );
        assert_eq!(back.value(1400.), 0.);
    }

    #[test]
    fn contrast_as_wcag() {
        assert!((contrast(rgb(0, 0, 0), rgb(255, 255, 255)) - 21.).abs() < 0.01);
        assert!((contrast(rgb(10, 10, 10), rgb(10, 10, 10)) - 1.).abs() < 0.001);
        assert!(is_dark(GREY_900) && !is_dark(rgb(0x80, 0x80, 0x80)));
        assert_eq!(max_contrast(rgb(0xfa, 0xfa, 0xfa)), GREY_900);
        assert_eq!(max_contrast(rgb(0x20, 0x20, 0x20)), WHITE);
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
    fn hover_fills() {
        let light = rgb(0xfa, 0xfa, 0xfa);
        assert_eq!(
            hover_fill(None, light, true),
            HOVER_LIGHT,
            "Chrome's baseline in light"
        );
        assert_eq!(hover_fill(None, rgb(0x20, 0x20, 0x20), true), HOVER_DARK);
        assert_eq!(
            hover_fill(Some(rgb(1, 2, 3)), light, true),
            rgb(1, 2, 3),
            "the theme's own when given"
        );
        let unfocused = hover_fill(Some(rgb(1, 2, 3)), light, false);
        assert!(
            unfocused.0 < light.0 && unfocused.0 > 0.9,
            "a faint neutral over the strip: {unfocused:?}"
        );
        let h = hover_at(rgb(255, 255, 255), rgb(0, 0, 0), 0.4);
        assert!((h.0 - 0.4).abs() < 1e-6);
    }
}
