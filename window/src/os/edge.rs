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

/// Where the edge's pictures come from.
enum Source {
    /// The desktop theme's, as pictures (the GTK frame).
    Pictures {
        focused: RgbaImage,
        unfocused: RgbaImage,
    },
    /// Chrome's own frame (chrome_strip::frame), drawn as needed.
    Chrome {
        /// With a Material shadow around the window (the window manager
        /// places the window by `_GTK_FRAME_EXTENTS`), else solid.
        shadow: bool,
        /// Round top corners (a compositor present), else square.
        round: bool,
    },
}

/// The window's edge, ready to draw.
pub struct Edge {
    config: WindowEdge,
    source: Source,
    /// The pictures at the scales (and, for Chrome's frame, the frame
    /// colours) last asked for, the most recent first: focused and
    /// unfocused windows ask for different frame colours, and a focus
    /// change must not redraw (and re-blur) the pictures.
    scaled: Vec<(f64, u32, RgbaImage, RgbaImage)>,
}

/// How many (scale, frame colour) picture pairs are kept.
const SCALED_KEPT: usize = 4;

impl Edge {
    /// The edge `config` asks for: Chrome's own frame with a `shadow`
    /// where the window manager takes `_GTK_FRAME_EXTENTS` (else solid)
    /// and `round` corners where a compositor runs; or the theme's
    /// pictures.
    pub fn new(config: &WindowEdge, shadow: bool, round: bool) -> anyhow::Result<Self> {
        if config.chrome {
            return Ok(Self {
                config: config.clone(),
                source: Source::Chrome { shadow, round },
                scaled: Vec::new(),
            });
        }
        Self::load(config)
    }

    /// The theme's pictures.
    pub fn load(config: &WindowEdge) -> anyhow::Result<Self> {
        let focused = image::open(&config.focused)?.to_rgba8();
        let unfocused = image::open(&config.unfocused)?.to_rgba8();
        anyhow::ensure!(
            focused.width() == focused.height() && focused.dimensions() == unfocused.dimensions(),
            "the edge pictures are not two squares of one size"
        );
        Ok(Self {
            config: config.clone(),
            source: Source::Pictures { focused, unfocused },
            scaled: Vec::new(),
        })
    }

    /// Chrome's solid frame: the border line's top run crosses the
    /// content's first row, and the top 4 DIP of the content resize.
    pub fn is_solid(&self) -> bool {
        matches!(self.source, Source::Chrome { shadow: false, .. })
    }

    /// The margins at `scale` (dpi / 96): the drawing's reach on each
    /// side, at least the resize band (Chrome's
    /// RestoredFrameBorderInsets); none when `restored` is false.
    pub fn insets(&self, scale: f64, restored: bool) -> Insets {
        if !restored {
            return Insets::default();
        }
        // Chrome ceils its frame extents to pixels
        let px = |v: f64| (v * scale).ceil().clamp(0., 255.) as u16;
        if let Source::Chrome { shadow, .. } = self.source {
            let r = chrome_strip::frame::reach(shadow);
            return Insets {
                left: px(r.left.into()),
                right: px(r.right.into()),
                top: px(r.top.into()),
                bottom: px(r.bottom.into()),
            };
        }
        let c = &self.config;
        Insets {
            left: px(c.left.max(c.input)),
            right: px(c.right.max(c.input)),
            top: px(c.top.max(c.input)),
            bottom: px(c.bottom.max(c.input)),
        }
    }

    /// A tiled window's insets: the resize band alone on every side.
    pub fn band_insets(&self, scale: f64) -> Insets {
        let band = self.band(scale);
        Insets {
            left: band,
            right: band,
            top: band,
            bottom: band,
        }
    }

    /// The margins cleared (a tiled window: no shadow), as `paint` gives
    /// them.
    pub fn clear(outer: (u16, u16), insets: Insets) -> Vec<(u16, u16, u16, u16, Vec<u8>)> {
        let (w, h) = outer;
        let rect = |x: u16, y: u16, rw: u16, rh: u16| {
            (
                x,
                y,
                rw,
                rh,
                vec![0u8; usize::from(rw) * usize::from(rh) * 4],
            )
        };
        let mut out = vec![];
        if insets.top > 0 {
            out.push(rect(0, 0, w, insets.top.min(h)));
        }
        if insets.bottom > 0 && h > insets.bottom {
            out.push(rect(0, h - insets.bottom, w, insets.bottom));
        }
        let middle = h.saturating_sub(insets.top + insets.bottom);
        if insets.left > 0 && middle > 0 {
            out.push(rect(0, insets.top, insets.left.min(w), middle));
        }
        if insets.right > 0 && middle > 0 && w > insets.right {
            out.push(rect(w - insets.right, insets.top, insets.right, middle));
        }
        out
    }

    /// The top corners' radius in pixels at `scale`.
    pub fn radius(&self, scale: f64) -> u16 {
        let dip = match self.source {
            Source::Chrome { round, .. } => {
                if round {
                    f64::from(chrome_strip::frame::RADIUS)
                } else {
                    0.
                }
            }
            Source::Pictures { .. } => self.config.radius,
        };
        (dip * scale).round().clamp(0., 64.) as u16
    }

    /// The resize band's width in pixels at `scale`: Chrome's 10 DIP on
    /// the shadow, its solid frame's own 4.
    pub fn band(&self, scale: f64) -> u16 {
        let dip = if self.is_solid() {
            f64::from(chrome_strip::frame::SOLID_BORDER)
        } else {
            self.config.input
        };
        (dip * scale).round().clamp(0., 64.) as u16
    }

    /// The pictures at `scale`, focused and not, the frame in `header`'s
    /// colour where they are Chrome's own.
    fn pictures(&mut self, scale: f64, header: [f32; 4]) -> (&RgbaImage, &RgbaImage) {
        let key = match self.source {
            Source::Chrome { .. } => {
                let c = header.map(|v| (v.clamp(0., 1.) * 255.).round() as u32);
                c[0] << 24 | c[1] << 16 | c[2] << 8 | c[3]
            }
            Source::Pictures { .. } => 0,
        };
        if let Some(i) = self.scaled.iter().position(|s| (s.0, s.1) == (scale, key)) {
            let hit = self.scaled.remove(i);
            self.scaled.insert(0, hit);
        } else {
            let side = ((4. * self.config.slice * scale).round() as u32).max(4) / 4 * 4;
            let (f, u) = match &self.source {
                Source::Pictures { focused, unfocused } => {
                    let resize = |p: &RgbaImage| {
                        image::imageops::resize(
                            p,
                            side,
                            side,
                            image::imageops::FilterType::CatmullRom,
                        )
                    };
                    (resize(focused), resize(unfocused))
                }
                Source::Chrome { shadow, round } => {
                    let frame = chrome_strip::SrgbaTuple(header[0], header[1], header[2], 1.);
                    let draw = |focused: bool| {
                        let p = chrome_strip::frame::picture(
                            self.config.slice as f32,
                            scale as f32,
                            focused,
                            *shadow,
                            *round,
                            frame,
                        );
                        RgbaImage::from_raw(p.side, p.side, p.rgba).expect("a square picture")
                    };
                    (draw(true), draw(false))
                }
            };
            self.scaled.insert(0, (scale, key, f, u));
            self.scaled.truncate(SCALED_KEPT);
        }
        let (_, _, f, u) = &self.scaled[0];
        (f, u)
    }

    /// The whole picture of an `outer`-sized window's edge, as one buffer
    /// of premultiplied BGRA (wl_shm ARGB8888), the content's own place in
    /// it clear but for the round top corners: what a subsurface beneath
    /// the content shows; into `out` (at least `outer`'s width x height
    /// x 4 bytes, rows packed), with no allocation of its own. (Wayland's)
    #[cfg_attr(not(feature = "wayland"), allow(dead_code))]
    pub fn paint_all_into(
        &mut self,
        focused: bool,
        outer: (u16, u16),
        insets: Insets,
        scale: f64,
        header: [f32; 4],
        out: &mut [u8],
    ) {
        let radius = self.radius(scale);
        let header_under = !self.is_solid();
        let (pf, pu) = self.pictures(scale, header);
        let picture = if focused { pf } else { pu };
        let geometry = Geometry::new(outer, insets, picture.width(), radius, header_under);
        let rect = (0, 0, outer.0, outer.1);
        geometry.fill(picture, header, rect, true, out);
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
        let header_under = !self.is_solid();
        let (pf, pu) = self.pictures(scale, header);
        let picture = if focused { pf } else { pu };
        let geometry = Geometry::new(outer, insets, picture.width(), radius, header_under);
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
                let mut data = vec![0; rw as usize * rh as usize * 4];
                geometry.fill(picture, header, (x, y, rw, rh), false, &mut data);
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
    /// Whether the header's colour goes under the picture in the
    /// content's round corners (a picture drawn around the content:
    /// the theme's, Chrome's shadow); Chrome's solid frame paints the
    /// corners itself.
    header_under: bool,
}

impl Geometry {
    fn new(
        outer: (u16, u16),
        insets: Insets,
        picture_side: u32,
        radius: u16,
        header_under: bool,
    ) -> Self {
        Self {
            insets,
            outer,
            slice: (picture_side / 4) as i32,
            radius,
            header_under,
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

    /// The window's pixels in `rect` (x, y, width, height) into `out`,
    /// rows packed: what `pixel` gives for each, the content's inside
    /// (but for its cut top corners) clear when `clear_inside`. Along a
    /// side the picture's middle is stretched, so every pixel between
    /// the corners is the same: each row computes its two ends and
    /// copies one value across the middle, and a row like the one before
    /// it (the same picture row, away from the round corners) is copied
    /// whole. A resize repaints in time proportional to the margins'
    /// length, not their area.
    fn fill(
        &self,
        picture: &RgbaImage,
        header: [f32; 4],
        rect: (u16, u16, u16, u16),
        clear_inside: bool,
        out: &mut [u8],
    ) {
        let (x, y, w, h) = rect;
        let (x, y, w, h) = (i32::from(x), i32::from(y), usize::from(w), i32::from(h));
        let row_len = w * 4;
        if row_len == 0 {
            return;
        }
        let (inner_w, inner_h) = self.inner();
        let (inner_w, inner_h) = (i32::from(inner_w), i32::from(inner_h));
        let (l, t) = (i32::from(self.insets.left), i32::from(self.insets.top));
        let r = i32::from(self.radius);
        // the middle run along x: the picture's middle column, no round
        // corner in it
        let run0 = self.slice.min(inner_w / 2).max(r);
        let run1 = inner_w - run0;
        let mut previous: Option<(i32, i32, bool)> = None;
        for (j, py) in (y..y + h).enumerate() {
            let dy = py - t;
            let inside_v = dy >= 0 && dy < inner_h;
            let key = (
                self.source(dy, inner_h),
                if (0..r).contains(&dy) { dy } else { -1 },
                inside_v,
            );
            let at = j * row_len;
            if j > 0 && previous == Some(key) {
                out.copy_within(at - row_len..at, at);
                continue;
            }
            previous = Some(key);
            let row = &mut out[at..at + row_len];
            let mut px = x;
            while px < x + w as i32 {
                let dx = px - l;
                let i = (px - x) as usize * 4;
                if dx >= run0 && dx < run1 {
                    let end = (run1 + l).min(x + w as i32);
                    let value = if clear_inside && inside_v {
                        [0; 4]
                    } else {
                        self.pixel(picture, px as u16, py as u16, header)
                    };
                    for chunk in row[i..(end - x) as usize * 4].as_chunks_mut::<4>().0 {
                        chunk.copy_from_slice(&value);
                    }
                    px = end;
                    continue;
                }
                let inside = inside_v && dx >= 0 && dx < inner_w;
                let corner = dy < r && (dx < r || dx >= inner_w - r.min(inner_w));
                let value = if clear_inside && inside && !corner {
                    [0; 4]
                } else {
                    self.pixel(picture, px as u16, py as u16, header)
                };
                row[i..i + 4].copy_from_slice(&value);
                px += 1;
            }
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
        let cover = if self.header_under {
            self.corner_cover(dx, dy, i32::from(inner_w))
        } else {
            0.
        };
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

/// How far in from each side a round top corner of `radius` pixels starts
/// on each of its rows, top first: the pixels whose centres lie outside
/// the corner's circle are left out.
pub fn corner_row_insets(radius: u16) -> Vec<u16> {
    let r = f32::from(radius);
    (0..radius)
        .map(|y| {
            let dy = r - f32::from(y) - 0.5;
            let half = (r * r - dy * dy).max(0.).sqrt();
            (r - half - 0.5).ceil().max(0.) as u16
        })
        .collect()
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
        let g = Geometry::new((440, 340), insets(), 256, 8, true);
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
        let g = Geometry::new((440, 340), insets(), 256, 8, true);
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

    /// The row-by-row fill gives what `pixel` gives for every pixel: the
    /// whole window (the content clear) and the margins' rectangles, at
    /// sizes around the corners' reach, round corners and not.
    #[test]
    fn fill_matches_every_pixel() {
        let mut picture = RgbaImage::new(64, 64);
        for (x, y, p) in picture.enumerate_pixels_mut() {
            *p = image::Rgba([
                (x * 4) as u8,
                (y * 4) as u8,
                (x ^ y) as u8 * 3,
                ((x + y) * 2) as u8,
            ]);
        }
        let header = [0.2, 0.4, 0.6, 1.];
        for (outer, radius, under) in [
            ((440, 340), 8, true),
            ((70, 60), 8, true),
            ((41, 41), 0, false),
            ((200, 90), 12, true),
            ((42, 200), 4, false),
        ] {
            let g = Geometry::new(outer, insets(), 64, radius, under);
            let (inner_w, inner_h) = g.inner();
            let (l, t) = (insets().left, insets().top);
            let mut expected = vec![];
            for y in 0..outer.1 {
                for x in 0..outer.0 {
                    let inside = x >= l && x < l + inner_w && y >= t && y < t + inner_h;
                    let corner = y < t + radius
                        && (x < l + radius || x >= l + inner_w - radius.min(inner_w));
                    if inside && !corner {
                        expected.extend([0; 4]);
                    } else {
                        expected.extend(g.pixel(&picture, x, y, header));
                    }
                }
            }
            let mut got = vec![0; expected.len()];
            g.fill(&picture, header, (0, 0, outer.0, outer.1), true, &mut got);
            assert!(got == expected, "whole window {:?} r {}", outer, radius);
            for rect in [
                (0, 0, outer.0, 13),
                (0, 13, 20, outer.1 - 13),
                (7, 3, 30, 25),
            ] {
                let mut expected = vec![];
                for y in rect.1..rect.1 + rect.3 {
                    for x in rect.0..rect.0 + rect.2 {
                        expected.extend(g.pixel(&picture, x, y, header));
                    }
                }
                let mut got = vec![0; expected.len()];
                g.fill(&picture, header, rect, false, &mut got);
                assert!(
                    got == expected,
                    "rect {:?} of {:?} r {}",
                    rect,
                    outer,
                    radius
                );
            }
        }
    }

    /// The rows of a round corner: inset most at the top, down to none by
    /// the corner's last row, each pixel kept whose centre lies inside the
    /// circle (the rest are what the frame beneath shows).
    #[test]
    fn a_corner_is_cut_along_its_arc() {
        let rows = corner_row_insets(14);
        assert_eq!(rows.len(), 14);
        assert!(rows.windows(2).all(|w| w[0] >= w[1]), "{:?}", rows);
        assert_eq!(*rows.last().unwrap(), 0);
        assert!(rows[0] >= 8 && rows[0] < 14, "{:?}", rows);
        let r = 14f32;
        for (y, &inset) in rows.iter().enumerate() {
            let dy = r - y as f32 - 0.5;
            let inside = |x: u16| (f32::from(x) + 0.5 - r).powi(2) + dy * dy <= r * r;
            assert!(inside(inset), "row {}: first kept pixel inside", y);
            if inset > 0 {
                assert!(!inside(inset - 1), "row {}: the one before outside", y);
            }
        }
        assert!(corner_row_insets(0).is_empty());
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
