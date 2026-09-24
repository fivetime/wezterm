//! Chrome's tab strip, as Chrome draws it over a desktop's own theme
//! (`tab_strip_style = "Chrome"`): its geometry (Chromium's
//! layout_constants.cc and tab_style.cc) and its colour rules
//! (tab_strip_color_mixer.cc, native_chrome_color_mixer_linux.cc,
//! BrowserView::ShouldDrawTabStrokes).
//!
//! The strip is the frame's colour; only the active tab is filled — with
//! the colour of what lies beneath it, which it joins through its
//! "feet" — the others show the strip, filled only under the pointer;
//! thin separators part the unfilled ones; and where the active tab's
//! colour is too close to the strip's to tell them apart, a one-pixel
//! stroke outlines it.

use crate::tabbar::TabBarItem;
use crate::termwindow::box_model::ComputedElement;
use crate::termwindow::UIItemType;
use config::SrgbaTuple;
use window::RectF;

/// Sizes in DIP (pixels at 96 dpi).
pub const STRIP_HEIGHT: f32 = 41.;
/// The tab shape starts this far below the strip's top.
pub const TAB_TOP: f32 = 6.;
pub const TOP_RADIUS: f32 = 10.;
/// The active tab's feet, flaring out at the bottom.
pub const FOOT: f32 = 12.;
/// A tab's body (between its feet): Chrome's standard 256 less the feet,
/// and its narrowest.
pub const BODY_MAX: f32 = 232.;
pub const BODY_MIN: f32 = 32.;
/// Between two tab bodies (Chrome's 18 of overlap of 2 × 12 feet).
pub const GAP: f32 = 6.;
/// The title's inset from the body's edge (Chrome's 20 from the tab's).
pub const INSET: f32 = 8.;
pub const SEPARATOR: (f32, f32) = (2., 16.);
/// The close and new-tab buttons: a circle around a 16-DIP icon.
pub const BUTTON: f32 = 28.;
/// Their hover highlight's opacity.
pub const HIGHLIGHT: f32 = 0.16;
/// A favicon's size and the room after it before the title
/// (gfx::kFaviconSize, kTabPreTitlePadding).
pub const FAVICON: f32 = 16.;
pub const FAVICON_GAP: f32 = 8.;
/// The strip's last DIP lies under the toolbar (kTabstripToolbarOverlap):
/// only `VISIBLE` of it shows, and the terminal begins there.
pub const OVERLAP: f32 = 1.;
pub const VISIBLE: f32 = STRIP_HEIGHT - OVERLAP;
/// A highlight's height: the tab's 35 less the strip's padding and the
/// toolbar overlap (kTabHeight - kTabStripPadding - kTabstripToolbarOverlap).
pub const HIGHLIGHT_HEIGHT: f32 = 28.;

/// The contrast below which the active tab gets its stroke, and the one
/// its stroke keeps from it.
const STROKE_BELOW: f32 = 1.3;
const STROKE_CONTRAST: f32 = 2.0;
/// An inactive tab under the pointer: this much of the active colour.
const HOVER: f32 = 0.4;

fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: SrgbaTuple) -> f32 {
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// WCAG's contrast ratio, as Chromium's `GetContrastRatio`.
pub fn contrast(a: SrgbaTuple, b: SrgbaTuple) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `a` over `b` at `alpha` (sRGB, as Chromium's `AlphaBlend`).
pub fn blend(a: SrgbaTuple, b: SrgbaTuple, alpha: f32) -> SrgbaTuple {
    let m = |x: f32, y: f32| x * alpha + y * (1. - alpha);
    SrgbaTuple(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2), 1.)
}

/// `fg` moved towards `target` only as far as it needs to reach `ratio`
/// against `bg` (Chromium's `BlendForMinContrast`); `None` if even
/// `target` does not.
fn min_contrast(
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

/// The active tab's stroke, where it would not stand out from the strip
/// otherwise: its own colour taken towards Google Grey 900 until it
/// contrasts 2:1 with it, else towards white.
pub fn stroke(active: SrgbaTuple, frame: SrgbaTuple) -> Option<SrgbaTuple> {
    if contrast(active, frame) >= STROKE_BELOW {
        return None;
    }
    let grey900 = SrgbaTuple(
        0x20 as f32 / 255.,
        0x21 as f32 / 255.,
        0x24 as f32 / 255.,
        1.,
    );
    let white = SrgbaTuple(1., 1., 1., 1.);
    min_contrast(active, active, grey900, STROKE_CONTRAST)
        .or_else(|| min_contrast(active, active, white, STROKE_CONTRAST))
}

/// An inactive tab under the pointer.
pub fn hover(active: SrgbaTuple, frame: SrgbaTuple) -> SrgbaTuple {
    blend(active, frame, HOVER)
}

/// Where the tab title's baseline lies, in pixels from the strip's top:
/// Chrome centres the cap height of a label spanning the whole 41-DIP tab
/// (RenderText::DetermineBaselineCenteringText), in whole pixels of the
/// font's rounded-up ascent, descent and cap height. Without a cap
/// height the whole font height is centred, as Chrome does.
pub fn title_baseline(ascent: f32, descent: f32, cap_height: Option<f32>, scale: f32) -> f32 {
    let a = ascent.ceil();
    let h = a + descent.ceil();
    // without a cap height Chrome centres the whole font height (its
    // GetCapHeight then answers the ascent: no internal leading)
    let (k, leading) = match cap_height {
        Some(c) => (c.ceil(), a - c.ceil()),
        None => (a, 0.),
    };
    let display = (STRIP_HEIGHT * scale).round();
    let space = display - if leading != 0. { k } else { h };
    let shift = (space / 2.).trunc() - leading;
    let (min, max) = ((display - h).min(0.), (display - h).abs());
    a + shift.clamp(min, max)
}

/// Where Chrome paints a hovered inactive tab (PathType::kHighlight): a
/// rounded rectangle from the body's top over its width, 28 DIP tall, its
/// corners the tab's top radius or less for a narrow tab (a third of the
/// top flat, GetTopCornerRadiusForWidth, over the width with the feet).
pub fn highlight(body: RectF, scale: f32) -> (RectF, f32) {
    let width = body.width() / scale + 2. * FOOT;
    let radius = ((width - 2. * TOP_RADIUS) / 3.).clamp(0., TOP_RADIUS) * scale;
    let height = (HIGHLIGHT_HEIGHT * scale).min(body.height());
    (
        euclid::rect(body.min_x(), body.min_y(), body.width(), height),
        radius,
    )
}

/// A tab body's width: the room for them all, within Chrome's limits.
pub fn body_width(room: f32, tabs: usize, scale: f32) -> f32 {
    let each = room / tabs.max(1) as f32 - GAP * scale;
    each.clamp(BODY_MIN * scale, BODY_MAX * scale)
}

/// The tabs in a computed tab bar: their bodies, whether active, left to
/// right.
pub fn tabs(element: &ComputedElement) -> Vec<(RectF, bool)> {
    fn walk(e: &ComputedElement, out: &mut Vec<(RectF, bool)>) {
        if let Some(UIItemType::TabBar(TabBarItem::Tab { active, .. })) = &e.item_type {
            out.push((e.border_rect, *active));
            return;
        }
        if let crate::termwindow::box_model::ComputedElementContent::Children(kids) = &e.content {
            for kid in kids {
                walk(kid, out);
            }
        }
    }
    let mut out = vec![];
    walk(element, &mut out);
    out.sort_by(|a, b| a.0.min_x().total_cmp(&b.0.min_x()));
    out
}

/// A tab's shape, `w` by `h` pixels with its feet: filled, or its outline
/// but for the bottom (the stroke), as a white mask. Quarter circles as
/// cubics (Chromium builds the same path with arcs).
pub fn shape(w: u32, h: u32, top: f32, foot: f32, outline: bool) -> window::bitmaps::Image {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let (wf, hf) = (w as f32, h as f32);
    let foot = foot.min(wf / 4.).min(hf / 2.);
    let top = top.min((wf - 2. * foot) / 2.).min(hf - foot).max(0.);
    let k = 0.552_284_8;
    let mut pb = PathBuilder::new();
    // an outline is drawn half a pixel in, with its radii half a pixel
    // smaller, so the stroke lies on the shape's edge (Chrome's stroke
    // adjustment)
    let (x0, x1, y0, tp, ft) = if outline {
        (
            0.5,
            wf - 0.5,
            0.5,
            (top - 0.5).max(0.),
            (foot - 0.5).max(0.),
        )
    } else {
        (0., wf, 0., top, foot)
    };
    if foot == 0. && !outline {
        // no feet: a rounded rectangle (the hover highlight)
        let r = top.min(wf / 2.).min(hf / 2.);
        pb.move_to(r, 0.);
        pb.line_to(wf - r, 0.);
        pb.cubic_to(wf - r + k * r, 0., wf, r - k * r, wf, r);
        pb.line_to(wf, hf - r);
        pb.cubic_to(wf, hf - r + k * r, wf - r + k * r, hf, wf - r, hf);
        pb.line_to(r, hf);
        pb.cubic_to(r - k * r, hf, 0., hf - r + k * r, 0., hf - r);
        pb.line_to(0., r);
        pb.cubic_to(0., r - k * r, r - k * r, 0., r, 0.);
        pb.close();
    } else {
        pb.move_to(x0, hf);
        pb.cubic_to(x0 + k * ft, hf, x0 + ft, hf - ft + k * ft, x0 + ft, hf - ft);
        pb.line_to(x0 + ft, y0 + tp);
        pb.cubic_to(
            x0 + ft,
            y0 + tp - k * tp,
            x0 + ft + tp - k * tp,
            y0,
            x0 + ft + tp,
            y0,
        );
        pb.line_to(x1 - ft - tp, y0);
        pb.cubic_to(
            x1 - ft - tp + k * tp,
            y0,
            x1 - ft,
            y0 + tp - k * tp,
            x1 - ft,
            y0 + tp,
        );
        pb.line_to(x1 - ft, hf - ft);
        pb.cubic_to(x1 - ft, hf - ft + k * ft, x1 - k * ft, hf, x1, hf);
        if !outline {
            pb.close();
        }
    }
    let mut image = window::bitmaps::Image::new(w.max(1) as usize, h.max(1) as usize);
    let (Some(path), Some(mut pixmap)) = (pb.finish(), Pixmap::new(w.max(1), h.max(1))) else {
        return image;
    };
    let mut paint = Paint::default();
    paint.set_color(tiny_skia::Color::WHITE);
    paint.anti_alias = true;
    if outline {
        let stroke = Stroke {
            width: 1.,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    } else {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mask: Vec<u8> = pixmap
        .data()
        .chunks_exact(4)
        .flat_map(|p| [p[3]; 4])
        .collect();
    image = window::bitmaps::Image::from_raw(w.max(1) as usize, h.max(1) as usize, mask);
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use window::bitmaps::BitmapImage;

    fn rgb(r: u8, g: u8, b: u8) -> SrgbaTuple {
        SrgbaTuple(r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.)
    }

    #[test]
    fn contrast_as_wcag() {
        assert!((contrast(rgb(0, 0, 0), rgb(255, 255, 255)) - 21.).abs() < 0.01);
        assert!((contrast(rgb(10, 10, 10), rgb(10, 10, 10)) - 1.).abs() < 0.001);
    }

    #[test]
    fn a_stroke_only_where_the_tab_would_not_show() {
        // elementary dark: the terminal's black on the header's grey shows
        assert_eq!(stroke(rgb(0x0c, 0x0c, 0x0c), rgb(0x2e, 0x2e, 0x2e)), None);
        // Lingmo: a light terminal on a light strip gets one, darker
        let s = stroke(rgb(0xfa, 0xfa, 0xfa), rgb(0xfa, 0xfa, 0xfa)).unwrap();
        assert!(contrast(s, rgb(0xfa, 0xfa, 0xfa)) >= 2.0 - 0.01);
        assert!(luminance(s) < luminance(rgb(0xfa, 0xfa, 0xfa)));
        // black on black: towards grey 900 is not enough, so towards white
        let s = stroke(rgb(0, 0, 0), rgb(0, 0, 0)).unwrap();
        assert!(luminance(s) > 0.);
    }

    #[test]
    fn tab_widths() {
        assert_eq!(
            body_width(1000., 2, 1.),
            BODY_MAX,
            "room to spare: Chrome's width"
        );
        assert_eq!(body_width(400., 4, 1.), 94., "shared out");
        assert_eq!(body_width(100., 10, 1.), BODY_MIN, "never narrower");
        assert_eq!(body_width(1000., 2, 2.), BODY_MAX * 2., "in pixels");
    }

    #[test]
    fn the_shape() {
        let s = shape(80, 30, 10., 12., false);
        let alpha = |x: usize, y: usize| s.pixel_data_slice()[(y * 80 + x) * 4 + 3];
        assert_eq!(alpha(40, 15), 255, "the body");
        assert_eq!(alpha(0, 0), 0, "outside the top corner");
        assert_eq!(alpha(10, 29), 255, "a foot reaches out along the bottom");
        assert!(alpha(1, 29) < 255, "tapering to its tip");
        assert_eq!(alpha(1, 10), 0, "above the foot, outside the body");
        let o = shape(80, 30, 10., 12., true);
        let oa = |x: usize, y: usize| o.pixel_data_slice()[(y * 80 + x) * 4 + 3];
        assert_eq!(oa(40, 15), 0, "an outline is hollow");
        assert!(oa(40, 0) > 0, "and runs along the top");
    }

    #[test]
    fn the_titles_baseline() {
        // Noto Sans CJK SC at 15 px: ascent 17.4, descent 4.3, cap 10.7 ->
        // Chrome's 18 + clamp((41 - 11) / 2 - (18 - 11), 0, 18) = 26
        assert_eq!(title_baseline(17.4, 4.3, Some(10.7), 1.), 26.);
        // no cap height: the font's whole height centred
        assert_eq!(
            title_baseline(17.4, 4.3, None, 1.),
            18. + 9.,
            "the 23-px font centred"
        );
        // a font taller than the tab is pushed no further than it must be
        assert_eq!(
            title_baseline(40., 10., Some(30.), 1.),
            35.,
            "a shift of 5 - 10, within the -9..9 the tab allows"
        );
    }

    #[test]
    fn the_highlight() {
        // a pill: no feet, every corner round
        let p = shape(80, 28, 10., 0., false);
        let alpha = |x: usize, y: usize| p.pixel_data_slice()[(y * 80 + x) * 4 + 3];
        assert_eq!(alpha(40, 14), 255);
        assert_eq!(alpha(0, 0), 0, "top corner");
        assert_eq!(alpha(0, 27), 0, "bottom corner too");
        assert_eq!(alpha(40, 27), 255, "flat along the bottom between");
        // over the body, 28 DIP from its top, corners 10 but a third flat
        let body = euclid::rect(100., 9., 300., 51.);
        let (rect, radius) = highlight(body, 1.5);
        assert_eq!(
            (rect.min_x(), rect.min_y(), rect.width(), rect.height()),
            (100., 9., 300., 42.)
        );
        assert_eq!(radius, 15.);
        let (_, narrow) = highlight(euclid::rect(0., 0., 12., 51.), 1.5);
        assert!(narrow < 15., "a narrow tab's corners smaller");
    }
}
