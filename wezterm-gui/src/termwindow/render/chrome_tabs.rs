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
    pb.move_to(0., hf);
    pb.cubic_to(k * foot, hf, foot, hf - foot + k * foot, foot, hf - foot);
    pb.line_to(foot, top);
    pb.cubic_to(
        foot,
        top - k * top,
        foot + top - k * top,
        0.,
        foot + top,
        0.,
    );
    pb.line_to(wf - foot - top, 0.);
    pb.cubic_to(
        wf - foot - top + k * top,
        0.,
        wf - foot,
        top - k * top,
        wf - foot,
        top,
    );
    pb.line_to(wf - foot, hf - foot);
    pb.cubic_to(wf - foot, hf - foot + k * foot, wf - k * foot, hf, wf, hf);
    if !outline {
        pb.close();
    }
    let mut image = window::bitmaps::Image::new(w.max(1) as usize, h.max(1) as usize);
    let (Some(path), Some(mut pixmap)) = (pb.finish(), Pixmap::new(w.max(1), h.max(1))) else {
        return image;
    };
    let mut paint = Paint::default();
    paint.set_color(tiny_skia::Color::WHITE);
    paint.anti_alias = true;
    if outline {
        // half a pixel in, so the stroke lies on the shape's edge
        let stroke = Stroke {
            width: 1.,
            ..Default::default()
        };
        pixmap.stroke_path(
            &path,
            &paint,
            &stroke,
            Transform::from_translate(0., 0.5),
            None,
        );
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
}
