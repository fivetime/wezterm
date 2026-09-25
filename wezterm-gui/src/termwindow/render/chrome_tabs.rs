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
use window::RectF;

/// The sizes and colour rules themselves live in the `chrome-strip` crate,
/// where they are held against Chromium's numbers without a window.
pub use chrome_strip::{
    hover_at, hover_fill, max_contrast, separator_opacity, stroke, Hover, CLOSE_HIGHLIGHT_RADIUS,
    FOOT, GAP, HIGHLIGHT_OPACITY as HIGHLIGHT, INSET, SEPARATOR_RADIUS, TOP_RADIUS,
};

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

/// A window's round top corner over its `r`-pixel corner square, as a
/// white mask: the quarter disc inside the window (its centre `r` in
/// and `r` down; the right corner mirrored), its edge sampled 4 x 4 as
/// the frame's own corners are (`chrome_strip::frame`). Multiplied
/// into the content's corner square, it leaves the content inside the
/// arc as drawn and clears what lies outside it, where the window's
/// own round corner is (see `TermWindow::mask_edge_corners`).
pub fn corner_mask(r: u32, right: bool) -> window::bitmaps::Image {
    let side = r.max(1) as usize;
    let rf = r as f32;
    let centre_x = if right { 0. } else { rf };
    let mut mask = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            // 4 x 4 samples per pixel for the arc's edge (as the frame's
            // own corners, `chrome_strip::frame`)
            let mut inside = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.;
                    if (px - centre_x).powi(2) + (py - rf).powi(2) <= rf * rf {
                        inside += 1;
                    }
                }
            }
            let alpha = (inside * 255 / 16) as u8;
            mask.extend([alpha; 4]);
        }
    }
    window::bitmaps::Image::from_raw(side, side, mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use window::bitmaps::BitmapImage;

    /// The corner mask keeps the inside of the arc and clears the
    /// outside: the corner pixel itself clear, the pixel by the centre
    /// full, the right corner the mirror image.
    #[test]
    fn the_corner_mask_is_the_quarter_disc_inside_the_window() {
        let alpha = |image: &window::bitmaps::Image, x: usize, y: usize| {
            image.pixel_data_slice()[(y * image.image_dimensions().0 + x) * 4 + 3]
        };
        let left = corner_mask(14, false);
        assert_eq!(left.image_dimensions(), (14, 14));
        assert_eq!(alpha(&left, 0, 0), 0, "outside the arc");
        assert_eq!(alpha(&left, 13, 13), 255, "by the centre");
        assert_eq!(
            alpha(&left, 13, 0),
            255,
            "the top edge reaches the corner square's far side"
        );
        // the arc's edge is anti-aliased somewhere (on the diagonal it
        // happens to fall between pixels: 3,3 out, 4,4 in)
        let partial = (0..14)
            .flat_map(|y| (0..14).map(move |x| (x, y)))
            .filter(|&(x, y)| matches!(alpha(&left, x, y), 1..=254))
            .count();
        assert!(partial >= 8, "anti-aliased edge pixels: {}", partial);
        assert_eq!(alpha(&left, 3, 3), 0);
        assert_eq!(alpha(&left, 4, 4), 255);
        let right = corner_mask(14, true);
        assert_eq!(alpha(&right, 13, 0), 0);
        assert_eq!(alpha(&right, 0, 13), 255);
        for y in 0..14 {
            for x in 0..14 {
                assert_eq!(
                    alpha(&left, x, y),
                    alpha(&right, 13 - x, y),
                    "mirrored at {x},{y}"
                );
            }
        }
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
    fn the_highlight() {
        // a pill: no feet, every corner round
        let p = shape(80, 28, 10., 0., false);
        let alpha = |x: usize, y: usize| p.pixel_data_slice()[(y * 80 + x) * 4 + 3];
        assert_eq!(alpha(40, 14), 255);
        assert_eq!(alpha(0, 0), 0, "top corner");
        assert_eq!(alpha(0, 27), 0, "bottom corner too");
        assert_eq!(alpha(40, 27), 255, "flat along the bottom between");
    }
}
