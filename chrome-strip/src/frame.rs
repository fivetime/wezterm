//! Chrome's own window frame on Linux, for a desktop whose toolkit gives
//! it none (a Qt one: "Qt prefers server-side decorations"), as
//! BrowserFrameViewLinux draws it (frame_view_utils_linux.cc,
//! shadow_value.cc, skia_paint_util.cc):
//!
//! - where the window manager knows `_GTK_FRAME_EXTENTS` and there is a
//!   compositor, a Material shadow around the window — elevation 16 when
//!   focused (a key shadow 16 DIP down, blurred 64, at 0x3d, and an
//!   ambient one blurred 32 at 0x1f), elevation 2 when not — a 1-pixel
//!   black border at 0x26 just outside the window, round top corners of
//!   radius 8, the frame's reach 10 above, 16 either side and 32 below
//!   (the shadows' extents, at least the 10-DIP resize border);
//! - else a solid frame: no shadow, 4 DIP of the frame's colour either
//!   side and below (part of the window, as the window manager sees it),
//!   the whole with round top corners, a 1-pixel contrasting border at
//!   0x26 just inside its outline (black or white, whichever contrasts
//!   more with the frame). The corners are square without compositing.
//!
//! `picture` draws either as a square the size of four slices, the window
//! in its middle two, which a nine-slicer cuts around any window.

use crate::SrgbaTuple;

/// One shadow: its offset down, its blur (Skia's: twice the CSS blur),
/// its alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub dy: f32,
    pub blur: f32,
    pub alpha: f32,
}

/// MakeMdShadowValues(elevation): the key shadow `elevation` down,
/// blurred 4 x elevation, at 0x3d; the ambient one in place, blurred
/// 2 x elevation, at 0x1f. Elevation 16 (Emphasis::kMaximum) focused, 2
/// (kMedium) not.
pub const fn md_shadows(elevation: f32) -> [Shadow; 2] {
    [
        Shadow {
            dy: elevation,
            blur: 4. * elevation,
            alpha: 0x3d as f32 / 255.,
        },
        Shadow {
            dy: 0.,
            blur: 2. * elevation,
            alpha: 0x1f as f32 / 255.,
        },
    ]
}
pub const ACTIVE_ELEVATION: f32 = 16.;
pub const INACTIVE_ELEVATION: f32 = 2.;
/// kResizeBorder, the solid frame's border (kFrameBorderThickness), the
/// top corners' radius (GetCornerRadiusMetric(Emphasis::kHigh)), the
/// border line's alpha (kBorderAlpha).
pub const RESIZE_BORDER: f32 = 10.;
pub const SOLID_BORDER: f32 = 4.;
pub const RADIUS: f32 = 8.;
pub const BORDER_ALPHA: f32 = 0x26 as f32 / 255.;

/// The frame's reach on each side, DIP.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reach {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// GetRestoredFrameBorderInsetsLinux: with a shadow, as far as the
/// shadows reach (a quarter of the blur, offset), at least the resize
/// border; without, 4 either side and below, nothing above.
pub fn reach(shadow: bool) -> Reach {
    if !shadow {
        return Reach {
            top: 0.,
            right: SOLID_BORDER,
            bottom: SOLID_BORDER,
            left: SOLID_BORDER,
        };
    }
    let (mut x0, mut y0, mut x1, mut y1) =
        (-RESIZE_BORDER, -RESIZE_BORDER, RESIZE_BORDER, RESIZE_BORDER);
    for s in md_shadows(ACTIVE_ELEVATION) {
        let r = s.blur / 4.;
        x0 = x0.min((-r).floor());
        x1 = x1.max(r.ceil());
        y0 = y0.min((-r + s.dy).floor());
        y1 = y1.max((r + s.dy).ceil());
    }
    Reach {
        top: -y0,
        right: x1,
        bottom: y1,
        left: -x0,
    }
}

/// Skia's blur radius to a Gaussian's sigma (SkiaUtils' RadiusToSigma),
/// for half the shadow's blur.
pub fn sigma(blur: f32) -> f32 {
    let radius = blur / 2.;
    if radius > 0. {
        0.288_675 * radius + 0.5
    } else {
        0.
    }
}

/// The solid frame's border line: black or white, whichever contrasts
/// more with the frame's colour (PickContrastingColor), at 0x26.
pub fn solid_border(frame: SrgbaTuple) -> SrgbaTuple {
    let v = if crate::contrast(crate::WHITE, frame)
        > crate::contrast(SrgbaTuple(0., 0., 0., 1.), frame)
    {
        1.
    } else {
        0.
    };
    SrgbaTuple(v, v, v, BORDER_ALPHA)
}

/// A square picture of the frame, straight RGBA, `side` pixels a side.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub side: u32,
    pub rgba: Vec<u8>,
}

/// The coverage of a rectangle with round top corners over a `side`
/// square, `inset` pixels in from `x0..x1`, `y0..y1` (negative: out).
fn rounded_rect(side: usize, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Vec<f32> {
    let mut mask = vec![0f32; side * side];
    for y in 0..side {
        for x in 0..side {
            // 4 x 4 samples per pixel for the round corners' edges
            let mut inside = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.;
                    if px < x0 || px >= x1 || py < y0 || py >= y1 {
                        continue;
                    }
                    let (cx, cy) = if px < x0 + r && py < y0 + r {
                        (x0 + r, y0 + r)
                    } else if px >= x1 - r && py < y0 + r {
                        (x1 - r, y0 + r)
                    } else {
                        inside += 1;
                        continue;
                    };
                    if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                        inside += 1;
                    }
                }
            }
            mask[y * side + x] = inside as f32 / 16.;
        }
    }
    mask
}

/// A Gaussian blur, separable, kernel out to three sigmas.
fn blur(mask: &[f32], side: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0. {
        return mask.to_vec();
    }
    let radius = (3. * sigma).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i as f32).powi(2) / (2. * sigma * sigma)).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / sum).collect();
    let mut tmp = vec![0f32; side * side];
    for y in 0..side {
        for x in 0..side {
            let mut acc = 0.;
            for (k, w) in kernel.iter().enumerate() {
                let sx = x as isize + k as isize - radius;
                if sx >= 0 && (sx as usize) < side {
                    acc += mask[y * side + sx as usize] * w;
                }
            }
            tmp[y * side + x] = acc;
        }
    }
    let mut out = vec![0f32; side * side];
    for y in 0..side {
        for x in 0..side {
            let mut acc = 0.;
            for (k, w) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius;
                if sy >= 0 && (sy as usize) < side {
                    acc += tmp[sy as usize * side + x] * w;
                }
            }
            out[y * side + x] = acc;
        }
    }
    out
}

/// The frame picture: `slice` DIP a slice at `scale` (the window the
/// middle two slices of four), for a focused window or not, with a shadow
/// or solid, the frame in `frame`'s colour (the solid borders), the top
/// corners `round` (a compositor present) or square.
pub fn picture(
    slice: f32,
    scale: f32,
    focused: bool,
    shadow: bool,
    round: bool,
    frame: SrgbaTuple,
) -> Picture {
    let side_px = ((4. * slice * scale).round() as u32).max(4) / 4 * 4;
    let side = side_px as usize;
    let s = side as f32 / 4.;
    let (x0, y0, x1, y1) = (s, s, 3. * s, 3. * s);
    let r = if round { RADIUS * scale } else { 0. };
    // straight RGBA over transparent, composited layer by layer
    let mut rgba = vec![0f32; side * side * 4];
    let over = |rgba: &mut [f32], i: usize, colour: (f32, f32, f32), a: f32| {
        if a <= 0. {
            return;
        }
        let da = rgba[i * 4 + 3];
        let out_a = a + da * (1. - a);
        for (c, v) in [colour.0, colour.1, colour.2].iter().enumerate() {
            let dc = rgba[i * 4 + c];
            rgba[i * 4 + c] = if out_a > 0. {
                (v * a + dc * da * (1. - a)) / out_a
            } else {
                0.
            };
        }
        rgba[i * 4 + 3] = out_a;
    };
    let window = rounded_rect(side, x0, y0, x1, y1, r);
    if shadow {
        // the shadows: the window's shape, a pixel out, blurred and moved
        // down, drawn only outside the window (the content covers it)
        let elevation = if focused {
            ACTIVE_ELEVATION
        } else {
            INACTIVE_ELEVATION
        };
        let outset = rounded_rect(side, x0 - 1., y0 - 1., x1 + 1., y1 + 1., r + 1.);
        for sh in md_shadows(elevation).iter().rev() {
            let blurred = blur(&outset, side, sigma(sh.blur * scale));
            let dy = (sh.dy * scale).round() as isize;
            for y in 0..side {
                for x in 0..side {
                    let sy = y as isize - dy;
                    if sy < 0 || sy as usize >= side {
                        continue;
                    }
                    let i = y * side + x;
                    let a = blurred[sy as usize * side + x] * sh.alpha * (1. - window[i]);
                    over(&mut rgba, i, (0., 0., 0.), a);
                }
            }
        }
        // the border: black at 0x26, the pixel just outside the window
        for i in 0..side * side {
            let a = (outset[i] - window[i]).max(0.) * BORDER_ALPHA;
            over(&mut rgba, i, (0., 0., 0.), a);
        }
    } else {
        // solid: the frame's colour 4 DIP either side and below, the
        // whole (borders and content) one round-cornered shape, the
        // contrasting line a pixel inside its outline; the content's own
        // place clear
        let b = (SOLID_BORDER * scale).round();
        let frame_rgb = (frame.0, frame.1, frame.2);
        let line = solid_border(frame);
        let outer = rounded_rect(side, x0 - b, y0, x1 + b, y1 + b, r);
        let inner = rounded_rect(
            side,
            x0 - b + 1.,
            y0 + 1.,
            x1 + b - 1.,
            y1 + b - 1.,
            (r - 1.).max(0.),
        );
        for i in 0..side * side {
            let a = outer[i] * (1. - window[i]);
            over(&mut rgba, i, frame_rgb, a);
            let ring = (outer[i] - inner[i]).max(0.) * (1. - window[i]);
            over(&mut rgba, i, (line.0, line.1, line.2), ring * line.3);
        }
    }
    Picture {
        side: side_px,
        rgba: rgba
            .iter()
            .map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromes_reach() {
        assert_eq!(
            reach(true),
            Reach {
                top: 10.,
                right: 16.,
                bottom: 32.,
                left: 16.
            },
            "the key shadow's 16 + 16 below, its 16 either side, the band above"
        );
        assert_eq!(
            reach(false),
            Reach {
                top: 0.,
                right: 4.,
                bottom: 4.,
                left: 4.
            }
        );
    }

    #[test]
    fn skias_sigma() {
        assert!((sigma(64.) - (0.288_675 * 32. + 0.5)).abs() < 1e-5);
        assert_eq!(sigma(0.), 0.);
    }

    fn alpha(p: &Picture, x: u32, y: u32) -> u8 {
        p.rgba[((y * p.side + x) * 4 + 3) as usize]
    }

    #[test]
    fn the_shadow_picture() {
        let frame = SrgbaTuple(0.95, 0.95, 0.95, 1.);
        let p = picture(64., 1., true, true, true, frame);
        assert_eq!(p.side, 256);
        // the window's middle is clear: the content goes there
        assert_eq!(alpha(&p, 128, 128), 0);
        // just outside the window's left edge: the border line and shadow
        let border = alpha(&p, 63, 128);
        assert!((0x26..0x90).contains(&border), "{border}");
        // the shadow fades outwards, deeper below than above
        let (near, far) = (alpha(&p, 40, 128), alpha(&p, 4, 128));
        assert!(near > far && far <= 2, "near {near} far {far}");
        assert!(
            alpha(&p, 128, 200) > alpha(&p, 128, 50),
            "the key shadow falls downwards"
        );
        // the round corner's outside carries shadow, its inside is clear
        assert_eq!(alpha(&p, 130, 130), 0);
        assert!(alpha(&p, 64, 64) > 0, "outside the corner's arc");
        // an unfocused window's shadow is slighter
        let q = picture(64., 1., false, true, true, frame);
        assert!(alpha(&q, 128, 200) < alpha(&p, 128, 200));
    }

    #[test]
    fn the_solid_picture() {
        let frame = SrgbaTuple(0.95, 0.95, 0.95, 1.);
        let p = picture(64., 1., true, false, true, frame);
        // 4 of the frame's colour either side and below, nothing above
        assert_eq!(alpha(&p, 62, 128), 255);
        assert_eq!(alpha(&p, 58, 128), 0);
        assert_eq!(alpha(&p, 128, 194), 255);
        assert_eq!(alpha(&p, 128, 60), 0, "nothing above");
        assert_eq!(alpha(&p, 128, 128), 0, "the content's place");
        // the outermost pixel carries the darker line, the next is the
        // frame's own colour
        let px = |x: u32, y: u32| &p.rgba[((y * 256 + x) * 4) as usize..][..3];
        assert!(
            px(60, 128)[0] < px(61, 128)[0],
            "the line {:?} then the frame {:?}",
            px(60, 128),
            px(61, 128)
        );
        assert!(px(61, 128).iter().all(|c| *c >= 230));
        // the round corner is the outline's, 4 out from the content's edge:
        // outside its arc clear, the arc's foot at the content's top
        assert_eq!(alpha(&p, 60, 64), 0);
        assert!(alpha(&p, 66, 64) > 0);
        // square corners without compositing
        let q = picture(64., 1., true, false, false, frame);
        assert_eq!(alpha(&q, 60, 64), 255);
        // at 2x, twice as wide
        let q = picture(64., 2., true, false, true, frame);
        assert_eq!(q.side, 512);
        assert_eq!(alpha(&q, 121, 256), 255);
        assert_eq!(alpha(&q, 119, 256), 0);
    }

    #[test]
    fn the_solid_line_contrasts() {
        assert_eq!(solid_border(SrgbaTuple(0.95, 0.95, 0.95, 1.)).0, 0.);
        assert_eq!(solid_border(SrgbaTuple(0.1, 0.1, 0.1, 1.)).0, 1.);
    }
}
