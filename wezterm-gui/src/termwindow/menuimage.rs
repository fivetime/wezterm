//! The popup menu as a picture, for a window of its own
//! (`WindowOps::show_popup`), where the menu drawn over the terminal's
//! window (`popupmenu.rs`) cannot leave it. Laid out and shaded as
//! Chrome's menus are, its numbers in DIPs (ui/views/controls/menu/
//! menu_config.{h,cc}, the values `MenuConfig::InitCommon` leaves):
//! - an item: `item_vertical_margin` 6 above and below its line (or its
//!   icon, 16), `item_horizontal_border_padding` 12 before and after
//! - the label after the icons' column, 16 wide, and
//!   DISTANCE_RELATED_LABEL_HORIZONTAL 12 (layout_provider.cc), where an
//!   item has an icon
//! - a separator: `separator_height` 17, its line `separator_thickness`
//!   1 in its middle, from edge to edge
//! - the corners' radius, ShapeContextTokens::kMenuRadius: 12
//!   (ShapeSysTokens::kMediumSmall), which is the space above the first
//!   and below the last line too (`rounded_menu_vertical_border_size`,
//!   unset, falls back to it; menu_scroll_view_container.cc)
//! - the shadow, `bubble_menu_shadow_elevation` 12
//!   (gfx::ShadowValue::MakeMdShadowValues): black at 0x3d, 12 below,
//!   blurred by 48, and black at 0x1f, blurred by 24; a blur is a
//!   Gaussian of sigma 0.57735 * blur / 2 + 0.5 (skia's
//!   ConvertRadiusToSigma). Chrome gives it half its blur of room around
//!   the menu (ShadowValue::GetMargin: 24, 12 above, 36 below), where a
//!   Gaussian still has a fiftieth of the shadow and the picture's edge
//!   showed as a line; here three sigmas, where nothing is left of it
//!
//! Without translucent windows (X11 with no compositing manager) the
//! corners are square and there is no shadow
//! (MenuConfig::CornerRadiusForMenu); a line around the menu then.

use config::keyassignment::PopupMenuEntry;
use std::rc::Rc;
use std::sync::Arc;
use termwiz::nerdfonts::NERD_FONTS;
use wezterm_font::{LoadedFont, RasterizedGlyph};
use window::PopupImage;

const ITEM_VERTICAL_MARGIN: f32 = 6.;
const ITEM_HORIZONTAL_PADDING: f32 = 12.;
const ICON_SIZE: f32 = 16.;
const ICON_LABEL_SPACING: f32 = 12.;
const SEPARATOR_HEIGHT: f32 = 17.;
const SEPARATOR_THICKNESS: f32 = 1.;
const CORNER_RADIUS: f32 = 12.;
/// Offset below, blur, alpha: the key shadow and the ambient one.
const SHADOWS: [(f32, f32, f32); 2] = [(12., 48., 61. / 255.), (0., 24., 31. / 255.)];
/// Around the menu for its shadow: left, top, right, bottom. Three
/// sigmas of the wider shadow (its sigma 14.4), less its offset above and
/// more below.
const SHADOW_MARGIN: (f32, f32, f32, f32) = (44., 32., 44., 56.);

/// The menu's colours, sRGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuColors {
    pub bg: [f32; 3],
    pub fg: [f32; 3],
}

impl MenuColors {
    /// Towards the text's colour from the background's, as the eye sees
    /// it (the shades `popupmenu.rs` draws with).
    fn mix(&self, t: f32) -> [f32; 3] {
        let one = |i: usize| self.bg[i] + (self.fg[i] - self.bg[i]) * t;
        [one(0), one(1), one(2)]
    }
}

struct Glyph {
    /// From the line's left and its top.
    x: i32,
    y: i32,
    glyph: RasterizedGlyph,
}

enum Row {
    Separator,
    Item {
        /// The entry's index in the menu.
        idx: usize,
        choosable: bool,
        dimmed: bool,
        icon: Vec<Glyph>,
        label: Vec<Glyph>,
    },
}

/// The menu laid out: its lines' glyphs shaped and rendered once, painted
/// again whenever another line is highlighted.
pub struct MenuLayout {
    rows: Vec<(Row, i32, i32)>,
    colors: MenuColors,
    translucent: bool,
    scale: f32,
    /// The picture's size, and the menu's place in it.
    size: (usize, usize),
    body: (usize, usize, usize, usize),
    label_x: i32,
    icon_x: i32,
    line_height: i32,
}

/// Called, from any thread, when fonts that were missing have been found.
pub type OnFonts = Arc<dyn Fn() + Send + Sync>;

fn shape(font: &Rc<LoadedFont>, text: &str, on_fonts: &OnFonts) -> (Vec<Glyph>, i32) {
    let metrics = font.metrics();
    let baseline = (metrics.cell_height.get() + metrics.descender.get()) as f32;
    let mut glyphs = vec![];
    let mut x = 0f32;
    if text.is_empty() {
        return (glyphs, 0);
    }
    // (fonts are looked up for characters the font has not: the menu is
    // laid out again when they are there)
    let again = {
        let on_fonts = Arc::clone(on_fonts);
        move || on_fonts()
    };
    // (the first attempt fails when the fonts' fallback was worked out
    // anew by it: "Font fallback recalculated", and the next succeeds)
    let mut attempt = font.shape(
        text,
        again.clone(),
        |_| {},
        None,
        wezterm_bidi::Direction::LeftToRight,
        None,
        None,
    );
    if attempt.is_err() {
        attempt = font.shape(
            text,
            again,
            |_| {},
            None,
            wezterm_bidi::Direction::LeftToRight,
            None,
            None,
        );
    }
    let infos = match attempt {
        Ok(infos) => infos,
        Err(err) => {
            log::error!("popup menu: shaping {text:?}: {err:#}");
            return (glyphs, 0);
        }
    };
    for info in infos {
        match font.rasterize_glyph(info.glyph_pos, info.font_idx) {
            Ok(glyph) if glyph.width > 0 && glyph.height > 0 => {
                glyphs.push(Glyph {
                    x: (x + (info.x_offset + glyph.bearing_x).get() as f32).round() as i32,
                    y: (baseline - (info.y_offset + glyph.bearing_y).get() as f32).round() as i32,
                    glyph,
                });
            }
            Ok(_) => {}
            Err(err) => log::error!("popup menu: a glyph of {text:?}: {err:#}"),
        }
        x += info.x_advance.get() as f32;
    }
    (glyphs, x.ceil() as i32)
}

impl MenuLayout {
    /// `on_fonts` is called when fonts that were missing have been
    /// found: the caller lays the menu out again.
    pub fn new(
        entries: &[PopupMenuEntry],
        font: &Rc<LoadedFont>,
        colors: MenuColors,
        dpi: usize,
        translucent: bool,
        on_fonts: OnFonts,
    ) -> Self {
        let scale = dpi as f32 / 96.;
        let px = |dip: f32| (dip * scale).round() as i32;
        let line_height = font.metrics().cell_height.get().ceil() as i32;
        let with_icons = entries.iter().any(|e| e.icon.is_some());

        let margin = if translucent {
            (
                px(SHADOW_MARGIN.0),
                px(SHADOW_MARGIN.1),
                px(SHADOW_MARGIN.2),
                px(SHADOW_MARGIN.3),
            )
        } else {
            (0, 0, 0, 0)
        };
        // (square, its line around it: the space above and below is what
        // a menu without round corners has, 4)
        let border_y = if translucent {
            px(CORNER_RADIUS)
        } else {
            px(4.)
        };
        let icon_x = px(ITEM_HORIZONTAL_PADDING);
        let label_x = if with_icons {
            icon_x + px(ICON_SIZE) + px(ICON_LABEL_SPACING)
        } else {
            icon_x
        };

        let mut rows = vec![];
        let mut y = margin.1 + border_y;
        let mut widest = 0;
        for (idx, entry) in entries.iter().enumerate() {
            if entry.separator {
                let height = px(SEPARATOR_HEIGHT);
                rows.push((Row::Separator, y, y + height));
                y += height;
                continue;
            }
            let (label, width) = shape(font, &entry.label, &on_fonts);
            widest = widest.max(width);
            let icon = entry
                .icon
                .as_deref()
                .and_then(|name| {
                    let found = NERD_FONTS.get(name).copied();
                    if found.is_none() {
                        log::error!("nerdfont {name} not found in NERD_FONTS");
                    }
                    found
                })
                .map(|c| {
                    let (glyphs, width) = shape(font, &c.to_string(), &on_fonts);
                    // in the middle of the icons' column
                    let shift = (px(ICON_SIZE) - width) / 2;
                    glyphs
                        .into_iter()
                        .map(|g| Glyph {
                            x: g.x + shift,
                            ..g
                        })
                        .collect()
                })
                .unwrap_or_default();
            let height = line_height.max(px(ICON_SIZE)) + 2 * px(ITEM_VERTICAL_MARGIN);
            rows.push((
                Row::Item {
                    idx,
                    choosable: entry.enabled && !entry.header,
                    dimmed: !entry.enabled || entry.header,
                    icon,
                    label,
                },
                y,
                y + height,
            ));
            y += height;
        }
        let body_width = label_x + widest + px(ITEM_HORIZONTAL_PADDING);
        let body_height = y + border_y - margin.1;
        Self {
            rows,
            colors,
            translucent,
            scale,
            size: (
                (margin.0 + body_width + margin.2) as usize,
                (margin.1 + body_height + margin.3) as usize,
            ),
            body: (
                margin.0 as usize,
                margin.1 as usize,
                body_width as usize,
                body_height as usize,
            ),
            label_x,
            icon_x,
            line_height,
        }
    }

    /// The entry whose line the picture's point is on, when it can be
    /// chosen.
    pub fn choosable_at(&self, x: isize, y: isize) -> Option<usize> {
        let (bx, _, bw, _) = self.body;
        if x < bx as isize || x >= (bx + bw) as isize {
            return None;
        }
        self.rows.iter().find_map(|(row, y0, y1)| match row {
            Row::Item {
                idx,
                choosable: true,
                ..
            } if y >= *y0 as isize && y < *y1 as isize => Some(*idx),
            _ => None,
        })
    }

    /// The menu with `selected` highlighted.
    pub fn paint(&self, selected: Option<usize>) -> PopupImage {
        let (width, height) = self.size;
        let mut canvas = Canvas {
            width: width as i32,
            height: height as i32,
            data: vec![0u8; width * height * 4],
        };
        let (bx, by, bw, bh) = (
            self.body.0 as i32,
            self.body.1 as i32,
            self.body.2 as i32,
            self.body.3 as i32,
        );
        let line = self.colors.mix(0.12);
        let radius = if self.translucent {
            CORNER_RADIUS * self.scale
        } else {
            0.
        };

        if self.translucent {
            for &(below, blur, alpha) in &SHADOWS {
                let sigma = 0.57735 * (blur * self.scale / 2.) + 0.5;
                canvas.shadow(
                    (
                        bx as f32,
                        by as f32 + below * self.scale,
                        bw as f32,
                        bh as f32,
                    ),
                    sigma,
                    alpha,
                );
            }
        }
        // the menu, its corners round; and its lines within those
        // corners
        let shape = Rounded {
            rect: (bx as f32, by as f32, bw as f32, bh as f32),
            radius,
        };
        canvas.fill(&shape, (bx, by, bw, bh), self.colors.bg, 1.);
        if !self.translucent {
            for (x, y, w, h) in [
                (bx, by, bw, 1),
                (bx, by + bh - 1, bw, 1),
                (bx, by, 1, bh),
                (bx + bw - 1, by, 1, bh),
            ] {
                canvas.fill(&shape, (x, y, w, h), line, 1.);
            }
        }
        for (row, y0, y1) in &self.rows {
            match row {
                Row::Separator => {
                    let thickness = ((SEPARATOR_THICKNESS * self.scale).round() as i32).max(1);
                    let y = y0 + (y1 - y0 - thickness) / 2;
                    canvas.fill(&shape, (bx, y, bw, thickness), line, 1.);
                }
                Row::Item {
                    idx,
                    dimmed,
                    icon,
                    label,
                    ..
                } => {
                    if selected == Some(*idx) {
                        canvas.fill(&shape, (bx, *y0, bw, y1 - y0), self.colors.mix(0.08), 1.);
                    }
                    let color = if *dimmed {
                        self.colors.mix(0.45)
                    } else {
                        self.colors.fg
                    };
                    let top = y0 + (y1 - y0 - self.line_height) / 2;
                    for g in icon {
                        canvas.glyph(bx + self.icon_x + g.x, top + g.y, &g.glyph, color);
                    }
                    for g in label {
                        canvas.glyph(bx + self.label_x + g.x, top + g.y, &g.glyph, color);
                    }
                }
            }
        }
        PopupImage {
            width,
            height,
            data: canvas.data,
            body: self.body,
        }
    }
}

/// A rectangle with round corners: how much of a pixel is within it.
struct Rounded {
    rect: (f32, f32, f32, f32),
    radius: f32,
}

impl Rounded {
    fn coverage(&self, x: i32, y: i32) -> f32 {
        let (rx, ry, rw, rh) = self.rect;
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        if px < rx || py < ry || px > rx + rw || py > ry + rh {
            return 0.;
        }
        if self.radius <= 0. {
            return 1.;
        }
        // the corner's circle, where the pixel is in a corner's square
        let cx = px.clamp(rx + self.radius, rx + rw - self.radius);
        let cy = py.clamp(ry + self.radius, ry + rh - self.radius);
        let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        (self.radius - distance + 0.5).clamp(0., 1.)
    }
}

/// The Gaussian's integral up to `x`.
fn gaussian(x: f32, sigma: f32) -> f32 {
    // (Abramowitz and Stegun's 7.1.26 for erf, within 1.5e-7)
    let z = x / (sigma * std::f32::consts::SQRT_2);
    let t = 1. / (1. + 0.3275911 * z.abs());
    let poly = t
        * (0.254_829_6
            + t * (-0.284_496_72 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))));
    let erf = 1. - poly * (-z * z).exp();
    0.5 * (1. + if z < 0. { -erf } else { erf })
}

/// Premultiplied BGRA.
struct Canvas {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

impl Canvas {
    fn at(&mut self, x: i32, y: i32) -> Option<&mut [u8]> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let i = (y * self.width + x) as usize * 4;
        Some(&mut self.data[i..i + 4])
    }

    /// `color` (sRGB) at `alpha` over what is there.
    fn over(&mut self, x: i32, y: i32, color: [f32; 3], alpha: f32) {
        let Some(p) = self.at(x, y) else {
            return;
        };
        let one =
            |dst: u8, src: f32| (src * alpha * 255. + dst as f32 * (1. - alpha)).round() as u8;
        p[0] = one(p[0], color[2]);
        p[1] = one(p[1], color[1]);
        p[2] = one(p[2], color[0]);
        p[3] = (alpha * 255. + p[3] as f32 * (1. - alpha)).round() as u8;
    }

    /// The rectangle `rect` (x, y, width, height), where it is within
    /// `shape`.
    fn fill(&mut self, shape: &Rounded, rect: (i32, i32, i32, i32), color: [f32; 3], alpha: f32) {
        for y in rect.1..rect.1 + rect.3 {
            for x in rect.0..rect.0 + rect.2 {
                let coverage = shape.coverage(x, y);
                if coverage > 0. {
                    self.over(x, y, color, alpha * coverage);
                }
            }
        }
    }

    /// The shadow of `rect`, black at `alpha`, blurred by a Gaussian of
    /// `sigma` (the rectangle's own blur; its corners' roundness is
    /// within the blur).
    fn shadow(&mut self, rect: (f32, f32, f32, f32), sigma: f32, alpha: f32) {
        let (rx, ry, rw, rh) = rect;
        let columns: Vec<f32> = (0..self.width)
            .map(|x| {
                let px = x as f32 + 0.5;
                gaussian(px - rx, sigma) - gaussian(px - (rx + rw), sigma)
            })
            .collect();
        for y in 0..self.height {
            let py = y as f32 + 0.5;
            let row = gaussian(py - ry, sigma) - gaussian(py - (ry + rh), sigma);
            if row * alpha < 1. / 512. {
                continue;
            }
            for x in 0..self.width {
                let a = alpha * row * columns[x as usize];
                if a >= 1. / 512. {
                    self.over(x, y, [0., 0., 0.], a);
                }
            }
        }
    }

    /// `glyph` with its top left at the point: its coverage in `color`,
    /// or its own colours (an emoji).
    fn glyph(&mut self, x: i32, y: i32, glyph: &RasterizedGlyph, color: [f32; 3]) {
        for gy in 0..glyph.height {
            for gx in 0..glyph.width {
                let i = (gy * glyph.width + gx) * 4;
                let (r, g, b, a) = (
                    glyph.data[i],
                    glyph.data[i + 1],
                    glyph.data[i + 2],
                    glyph.data[i + 3],
                );
                if a == 0 {
                    continue;
                }
                let (px, py) = (x + gx as i32, y + gy as i32);
                if glyph.has_color {
                    // premultiplied already
                    let Some(p) = self.at(px, py) else {
                        continue;
                    };
                    let keep = 1. - a as f32 / 255.;
                    let one = |dst: u8, src: u8| (src as f32 + dst as f32 * keep).min(255.) as u8;
                    p[0] = one(p[0], b);
                    p[1] = one(p[1], g);
                    p[2] = one(p[2], r);
                    p[3] = one(p[3], a);
                } else {
                    self.over(px, py, color, a as f32 / 255.);
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn the_corners_are_round() {
        let shape = Rounded {
            rect: (10., 10., 100., 50.),
            radius: 12.,
        };
        assert_eq!(shape.coverage(0, 0), 0., "outside");
        assert_eq!(shape.coverage(10, 10), 0., "the corner's tip is cut");
        assert_eq!(shape.coverage(60, 10), 1., "the top edge's middle");
        assert_eq!(shape.coverage(60, 35), 1., "inside");
        assert_eq!(shape.coverage(109, 59), 0., "the opposite corner's tip");
        let edge = shape.coverage(13, 13);
        assert!(edge > 0. && edge < 1., "on the corner's circle: {}", edge);
        let square = Rounded {
            rect: (10., 10., 100., 50.),
            radius: 0.,
        };
        assert_eq!(square.coverage(10, 10), 1.);
    }

    #[test]
    fn the_blur_is_a_gaussian() {
        assert!((gaussian(0., 5.) - 0.5).abs() < 1e-6);
        assert!((gaussian(5., 5.) - 0.841345).abs() < 1e-4, "one sigma");
        assert!((gaussian(-10., 5.) - 0.022750).abs() < 1e-4, "two below");
        assert!(gaussian(100., 5.) > 0.999999);
    }

    #[test]
    fn a_shadow_fades_away_from_its_rectangle() {
        let mut canvas = Canvas {
            width: 100,
            height: 100,
            data: vec![0; 100 * 100 * 4],
        };
        canvas.shadow((30., 30., 40., 40.), 5., 0.5);
        let alpha = |canvas: &Canvas, x: usize, y: usize| canvas.data[(y * 100 + x) * 4 + 3];
        let (middle, edge, away) = (
            alpha(&canvas, 50, 50),
            alpha(&canvas, 30, 50),
            alpha(&canvas, 5, 50),
        );
        assert!((middle as i32 - 127).abs() <= 2, "{}", middle);
        // (the pixel's middle is half a pixel within the rectangle: the
        // Gaussian's integral a tenth of a sigma past its middle, 0.54)
        assert!((edge as i32 - 69).abs() <= 1, "at the edge: {}", edge);
        assert_eq!(away, 0);
        // black, premultiplied
        assert_eq!(&canvas.data[(50 * 100 + 50) * 4..][..3], &[0, 0, 0]);
    }

    #[test]
    fn the_shades_are_between_the_two_colours() {
        let colors = MenuColors {
            bg: [1., 1., 1.],
            fg: [0., 0., 0.],
        };
        assert_eq!(colors.mix(0.), [1., 1., 1.]);
        assert_eq!(colors.mix(1.), [0., 0., 0.]);
        assert!((colors.mix(0.12)[0] - 0.88).abs() < 1e-6);
    }
}
