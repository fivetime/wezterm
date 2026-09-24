//! A window's own edge, drawn by the window as the desktop theme draws it
//! (client-side decoration, the way Chrome does it on Linux): the X
//! window grows by margins around the content, the theme's border and
//! shadow are painted into them from a picture cut in nine (Chromium's
//! WindowFrameProviderGtk::PaintWindowFrame), the content's top corners
//! are cut round, `_GTK_FRAME_EXTENTS` tells the window manager where the
//! window really is, and the input shape lets clicks on the shadow fall
//! through but for a band that resizes. Maximized or full screen, none
//! of it: the content fills the window.
//!
//! This file is the arithmetic; window.rs does the X requests.

use config::WindowEdge;
use image::RgbaImage;

/// Margins in pixels: left, right, top, bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

impl Insets {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Which way a press on the margins resizes, as `ResizeEdge`.
pub use crate::ResizeEdge as Side;

/// The theme's edge, loaded.
pub struct Edge {
    config: WindowEdge,
    focused: RgbaImage,
    unfocused: RgbaImage,
    /// The pictures at the last scale asked for, and that scale.
    scaled: Option<(f64, RgbaImage, RgbaImage)>,
}

impl Edge {
    pub fn load(config: &WindowEdge) -> anyhow::Result<Self> {
        let focused = image::open(&config.focused)?.to_rgba8();
        let unfocused = image::open(&config.unfocused)?.to_rgba8();
        anyhow::ensure!(
            focused.width() == focused.height() && focused.dimensions() == unfocused.dimensions(),
            "the edge pictures are not two squares of one size"
        );
        Ok(Self {
            config: config.clone(),
            focused,
            unfocused,
            scaled: None,
        })
    }

    /// The margins at `scale` (dpi / 96): the drawing's reach on each
    /// side, at least the resize band (Chrome's
    /// RestoredFrameBorderInsets); none when `restored` is false.
    pub fn insets(&self, scale: f64, restored: bool) -> Insets {
        if !restored {
            return Insets::default();
        }
        let c = &self.config;
        let px = |v: f64| (v.max(c.input) * scale).round().clamp(0., 255.) as u16;
        Insets {
            left: px(c.left),
            right: px(c.right),
            top: px(c.top),
            bottom: px(c.bottom),
        }
    }

    /// The top corners' radius in pixels at `scale`.
    pub fn radius(&self, scale: f64) -> u16 {
        (self.config.radius * scale).round().clamp(0., 64.) as u16
    }

    /// The resize band's width in pixels at `scale`.
    pub fn band(&self, scale: f64) -> u16 {
        (self.config.input * scale).round().clamp(0., 64.) as u16
    }

    fn pictures(&mut self, scale: f64) -> (&RgbaImage, &RgbaImage) {
        if self.scaled.as_ref().map(|s| s.0) != Some(scale) {
            let side = ((4. * self.config.slice * scale).round() as u32).max(4) / 4 * 4;
            let resize = |p: &RgbaImage| {
                image::imageops::resize(p, side, side, image::imageops::FilterType::CatmullRom)
            };
            self.scaled = Some((scale, resize(&self.focused), resize(&self.unfocused)));
        }
        let (_, f, u) = self.scaled.as_ref().expect("scaled above");
        (f, u)
    }

    /// The whole picture of an `outer`-sized window's edge, as one buffer
    /// of premultiplied BGRA (wl_shm ARGB8888), the content's own place in
    /// it clear but for the round top corners: what a subsurface beneath
    /// the content shows.
    pub fn paint_all(
        &mut self,
        focused: bool,
        outer: (u16, u16),
        insets: Insets,
        scale: f64,
        header: [f32; 4],
    ) -> Vec<u8> {
        let radius = self.radius(scale);
        let (pf, pu) = self.pictures(scale);
        let picture = if focused { pf } else { pu };
        let geometry = Geometry::new(outer, insets, picture.width(), radius);
        let (inner_w, inner_h) = geometry.inner();
        let (l, t) = (insets.left, insets.top);
        let mut data = Vec::with_capacity(usize::from(outer.0) * usize::from(outer.1) * 4);
        for y in 0..outer.1 {
            for x in 0..outer.0 {
                let inside = x >= l && x < l + inner_w && y >= t && y < t + inner_h;
                let corner =
                    y < t + radius && (x < l + radius || x >= l + inner_w - radius.min(inner_w));
                if inside && !corner {
                    data.extend([0; 4]);
                } else {
                    data.extend(geometry.pixel(picture, x, y, header));
                }
            }
        }
        data
    }

    /// The pixels of the margins (and the rounded top corners) of an
    /// `outer`-sized window with `insets` around its content, as
    /// rectangles of premultiplied BGRA (a 32-bit X visual's order on a
    /// little-endian host): x, y, width, height, data.
    pub fn paint(
        &mut self,
        focused: bool,
        outer: (u16, u16),
        insets: Insets,
        scale: f64,
        header: [f32; 4],
    ) -> Vec<(u16, u16, u16, u16, Vec<u8>)> {
        let radius = self.radius(scale);
        let (pf, pu) = self.pictures(scale);
        let picture = if focused { pf } else { pu };
        let geometry = Geometry::new(outer, insets, picture.width(), radius);
        let (w, h) = outer;
        let (inner_w, inner_h) = geometry.inner();
        if inner_w == 0 || inner_h == 0 {
            return vec![];
        }
        // the margins, and the band under the content's top edge where
        // the corners are cut
        let top_rows = (insets.top + radius.min(inner_h)).min(h);
        let mut rects = vec![(0, 0, w, top_rows)];
        let bottom = insets.top + inner_h;
        if bottom < h {
            rects.push((0, bottom, w, h - bottom));
        }
        if top_rows < bottom {
            rects.push((0, top_rows, insets.left, bottom - top_rows));
            rects.push((
                insets.left + inner_w,
                top_rows,
                w - insets.left - inner_w,
                bottom - top_rows,
            ));
        }
        rects
            .into_iter()
            .filter(|r| r.2 > 0 && r.3 > 0)
            .map(|(x, y, rw, rh)| {
                let mut data = Vec::with_capacity(rw as usize * rh as usize * 4);
                for py in y..y + rh {
                    for px in x..x + rw {
                        data.extend(geometry.pixel(picture, px, py, header));
                    }
                }
                (x, y, rw, rh, data)
            })
            .collect()
    }
}

/// Where things are in one window at one scale.
struct Geometry {
    insets: Insets,
    outer: (u16, u16),
    /// The picture's corner size: the window sits in its middle
    /// `2 * slice`.
    slice: i32,
    radius: u16,
}

impl Geometry {
    fn new(outer: (u16, u16), insets: Insets, picture_side: u32, radius: u16) -> Self {
        Self {
            insets,
            outer,
            slice: (picture_side / 4) as i32,
            radius,
        }
    }

    fn inner(&self) -> (u16, u16) {
        let i = self.insets;
        (
            self.outer.0.saturating_sub(i.left + i.right),
            self.outer.1.saturating_sub(i.top + i.bottom),
        )
    }

    /// Where a window pixel `d` (relative to the content's edge, of an
    /// `extent`-long content side) comes from along one axis of the
    /// picture: corners kept as they are, the middle stretched from the
    /// picture's middle.
    fn source(&self, d: i32, extent: i32) -> i32 {
        let fs = self.slice;
        let corner = fs.min(extent / 2);
        if d < corner {
            fs + d
        } else if d >= extent - corner {
            3 * fs + (d - extent)
        } else {
            2 * fs
        }
    }

    /// The premultiplied BGRA pixel at window pixel (x, y): the theme's
    /// drawing over the header's colour in the content's cut top corners.
    fn pixel(&self, picture: &RgbaImage, x: u16, y: u16, header: [f32; 4]) -> [u8; 4] {
        let (inner_w, inner_h) = self.inner();
        let dx = i32::from(x) - i32::from(self.insets.left);
        let dy = i32::from(y) - i32::from(self.insets.top);
        let side = picture.width() as i32;
        let sx = self.source(dx, i32::from(inner_w)).clamp(0, side - 1);
        let sy = self.source(dy, i32::from(inner_h)).clamp(0, side - 1);
        let p = picture.get_pixel(sx as u32, sy as u32).0;
        let a = f32::from(p[3]) / 255.;
        // the picture is straight RGBA; X wants it premultiplied
        let mut out = [
            f32::from(p[2]) / 255. * a,
            f32::from(p[1]) / 255. * a,
            f32::from(p[0]) / 255. * a,
            a,
        ];
        let cover = self.corner_cover(dx, dy, i32::from(inner_w));
        if cover > 0. {
            // the header's colour under the drawing, as far as the round
            // corner covers the pixel
            let ha = header[3] * cover;
            let under = [header[2] * ha, header[1] * ha, header[0] * ha, ha];
            for i in 0..4 {
                out[i] += under[i] * (1. - a);
            }
        }
        out.map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
    }

    /// How much of content pixel (dx, dy) a top corner's rounding covers
    /// (1 inside, 0 outside, between on the curve); 0 away from the
    /// corners.
    fn corner_cover(&self, dx: i32, dy: i32, inner_w: i32) -> f32 {
        let r = i32::from(self.radius);
        if r == 0 || dy < 0 || dy >= r || dx < 0 || dx >= inner_w {
            return 0.;
        }
        let cx = if dx < r {
            r
        } else if dx >= inner_w - r {
            inner_w - r
        } else {
            return 0.;
        };
        let (px, py) = (dx as f32 + 0.5, dy as f32 + 0.5);
        let (ox, oy) = (px - cx as f32, py - r as f32);
        // only the quarter towards the corner is round
        if (dx < r && ox > 0.) || (dx >= inner_w - r && ox < 0.) || oy > 0. {
            return 1.;
        }
        let distance = (ox * ox + oy * oy).sqrt();
        (r as f32 - distance + 0.5).clamp(0., 1.)
    }
}

/// Which way a press at window pixel (x, y) resizes: in the resize band
/// just outside the content, corners reaching `corner` along the edges;
/// `None` on the content or past the band.
pub fn side_at(x: i32, y: i32, outer: (u16, u16), insets: Insets, band: u16) -> Option<Side> {
    let (l, t) = (i32::from(insets.left), i32::from(insets.top));
    let r = i32::from(outer.0) - i32::from(insets.right);
    let b = i32::from(outer.1) - i32::from(insets.bottom);
    let band = i32::from(band);
    if x >= l && x < r && y >= t && y < b {
        return None;
    }
    if x < l - band || x >= r + band || y < t - band || y >= b + band {
        return None;
    }
    // Chrome's corners reach 16 DIP along the edges for its 10-DIP band
    let corner = (f64::from(band) * 1.6).round() as i32;
    let (left, right) = (x < l + corner, x >= r - corner);
    let (top, bottom) = (y < t + corner, y >= b - corner);
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => Side::TopLeft,
        (_, true, true, _) => Side::TopRight,
        (true, _, _, true) => Side::BottomLeft,
        (_, true, _, true) => Side::BottomRight,
        _ if y < t => Side::Top,
        _ if y >= b => Side::Bottom,
        _ if x < l => Side::Left,
        _ => Side::Right,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insets() -> Insets {
        Insets {
            left: 20,
            right: 20,
            top: 10,
            bottom: 30,
        }
    }

    #[test]
    fn nine_slices() {
        // a 256 picture: slice 64, the window at 64..192
        let g = Geometry::new((440, 340), insets(), 256, 8);
        assert_eq!(g.inner(), (400, 300));
        assert_eq!(
            g.source(-20, 400),
            44,
            "the margin: outside the window's edge"
        );
        assert_eq!(g.source(0, 400), 64);
        assert_eq!(g.source(63, 400), 127, "the corner kept as it is");
        assert_eq!(g.source(200, 400), 128, "the middle stretched");
        assert_eq!(g.source(399, 400), 191, "the far corner");
        assert_eq!(g.source(419, 400), 211, "the far margin");
        assert_eq!(
            g.source(10, 40),
            74,
            "a small window: corners of half its size"
        );
        assert_eq!(g.source(30, 40), 182);
    }

    #[test]
    fn round_corners() {
        let g = Geometry::new((440, 340), insets(), 256, 8);
        assert_eq!(g.corner_cover(0, 0, 400), 0., "the very corner is cut");
        assert_eq!(g.corner_cover(7, 7, 400), 1., "inside the round");
        assert_eq!(g.corner_cover(399, 0, 400), 0., "the right corner too");
        assert_eq!(
            g.corner_cover(20, 0, 400),
            0.,
            "away from the corners: the content's own"
        );
        assert_eq!(g.corner_cover(3, 8, 400), 0., "below the rounding");
        let edge = g.corner_cover(2, 2, 400);
        assert!(edge > 0. && edge < 1., "on the curve: partly, {}", edge);
    }

    #[test]
    fn resize_band() {
        let (outer, i) = ((440, 340), insets());
        assert_eq!(side_at(100, 100, outer, i, 10), None, "the content");
        assert_eq!(side_at(100, 5, outer, i, 10), Some(Side::Top));
        assert_eq!(side_at(12, 100, outer, i, 10), Some(Side::Left));
        assert_eq!(
            side_at(5, 100, outer, i, 10),
            None,
            "past the band: the shadow"
        );
        assert_eq!(side_at(425, 100, outer, i, 10), Some(Side::Right));
        assert_eq!(side_at(100, 315, outer, i, 10), Some(Side::Bottom));
        assert_eq!(side_at(15, 5, outer, i, 10), Some(Side::TopLeft));
        assert_eq!(
            side_at(30, 5, outer, i, 10),
            Some(Side::TopLeft),
            "a corner reaches along the edge"
        );
        assert_eq!(side_at(425, 315, outer, i, 10), Some(Side::BottomRight));
        assert_eq!(
            side_at(100, 339, outer, i, 10),
            None,
            "past the bottom band"
        );
    }
}
