#![allow(dead_code)]

use crate::color::LinearRgba;
use crate::customglyph::{BlockKey, Poly};
use crate::glyphcache::CachedGlyph;
use crate::quad::{QuadImpl, QuadTrait, TripleLayerQuadAllocator, TripleLayerQuadAllocatorTrait};
use crate::termwindow::{
    ColorEase, MouseCapture, RenderState, TermWindowNotif, UIItem, UIItemType,
};
use crate::utilsprites::RenderMetrics;
use ::window::{RectF, WindowOps};
use anyhow::anyhow;
use config::{Dimension, DimensionContext};
use finl_unicode::grapheme_clusters::Graphemes;
use std::cell::RefCell;
use std::rc::Rc;
use termwiz::cell::{grapheme_column_width, Presentation};
use termwiz::surface::Line;
use wezterm_font::units::PixelUnit;
use wezterm_font::LoadedFont;
use wezterm_term::color::{ColorAttribute, ColorPalette};
use window::bitmaps::atlas::Sprite;
use window::bitmaps::TextureRect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Bottom,
    Middle,
}

impl Default for VerticalAlign {
    fn default() -> VerticalAlign {
        VerticalAlign::Top
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    Block,
    Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    None,
    Right,
}

impl Default for Float {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelDimension {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelSizedPoly {
    pub poly: &'static [Poly],
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SizedPoly {
    pub poly: &'static [Poly],
    pub width: Dimension,
    pub height: Dimension,
}

impl SizedPoly {
    pub fn to_pixels(&self, context: &LayoutContext) -> PixelSizedPoly {
        PixelSizedPoly {
            poly: self.poly,
            width: self.width.evaluate_as_pixels(context.width),
            height: self.height.evaluate_as_pixels(context.height),
        }
    }

    pub fn none() -> Self {
        Self {
            poly: &[],
            width: Dimension::default(),
            height: Dimension::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelCorners {
    pub top_left: PixelSizedPoly,
    pub top_right: PixelSizedPoly,
    pub bottom_left: PixelSizedPoly,
    pub bottom_right: PixelSizedPoly,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Corners {
    pub top_left: SizedPoly,
    pub top_right: SizedPoly,
    pub bottom_left: SizedPoly,
    pub bottom_right: SizedPoly,
}

impl Corners {
    pub fn to_pixels(&self, context: &LayoutContext) -> PixelCorners {
        PixelCorners {
            top_left: self.top_left.to_pixels(context),
            top_right: self.top_right.to_pixels(context),
            bottom_left: self.bottom_left.to_pixels(context),
            bottom_right: self.bottom_right.to_pixels(context),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxDimension {
    pub left: Dimension,
    pub top: Dimension,
    pub right: Dimension,
    pub bottom: Dimension,
}

impl BoxDimension {
    pub const fn new(dim: Dimension) -> Self {
        Self {
            left: dim,
            top: dim,
            right: dim,
            bottom: dim,
        }
    }

    pub fn to_pixels(&self, context: &LayoutContext) -> PixelDimension {
        PixelDimension {
            left: self.left.evaluate_as_pixels(context.width),
            top: self.top.evaluate_as_pixels(context.height),
            right: self.right.evaluate_as_pixels(context.width),
            bottom: self.bottom.evaluate_as_pixels(context.height),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InheritableColor {
    Inherited,
    Color(LinearRgba),
    Animated {
        color: LinearRgba,
        alt_color: LinearRgba,
        ease: Rc<RefCell<ColorEase>>,
        one_shot: bool,
    },
}

impl Default for InheritableColor {
    fn default() -> Self {
        Self::Inherited
    }
}

impl From<LinearRgba> for InheritableColor {
    fn from(color: LinearRgba) -> InheritableColor {
        InheritableColor::Color(color)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderColor {
    pub left: LinearRgba,
    pub top: LinearRgba,
    pub right: LinearRgba,
    pub bottom: LinearRgba,
}

impl BorderColor {
    pub const fn new(color: LinearRgba) -> Self {
        Self {
            left: color,
            top: color,
            right: color,
            bottom: color,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ElementColors {
    pub border: BorderColor,
    pub bg: InheritableColor,
    pub text: InheritableColor,
}

impl ElementColors {
    /// These colours with what they inherit filled in from `parent`, so
    /// that a child of this element inherits through it: an element
    /// that only groups its children (no colours of its own) passes its
    /// parent's on, rather than nothing.
    fn inherit_from(&self, parent: Option<&ElementColors>) -> ElementColors {
        let Some(parent) = parent else {
            return self.clone();
        };
        let pick = |own: &InheritableColor, theirs: &InheritableColor| match own {
            InheritableColor::Inherited => theirs.clone(),
            other => other.clone(),
        };
        ElementColors {
            border: self.border,
            bg: pick(&self.bg, &parent.bg),
            text: pick(&self.text, &parent.text),
        }
    }
}

struct ResolvedColor {
    color: LinearRgba,
    alt_color: LinearRgba,
    mix_value: f32,
}

impl ResolvedColor {
    fn apply(&self, quad: &mut QuadImpl) {
        quad.set_fg_color(self.color);
        quad.set_alt_color_and_mix_value(self.alt_color, self.mix_value);
    }
}

impl From<LinearRgba> for ResolvedColor {
    fn from(color: LinearRgba) -> Self {
        Self {
            color,
            alt_color: color,
            mix_value: 0.,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Element {
    pub item_type: Option<UIItemType>,
    pub vertical_align: VerticalAlign,
    pub zindex: i8,
    pub display: DisplayType,
    pub float: Float,
    pub padding: BoxDimension,
    pub margin: BoxDimension,
    pub border: BoxDimension,
    pub border_corners: Option<Corners>,
    pub colors: ElementColors,
    pub hover_colors: Option<ElementColors>,
    pub font: Rc<LoadedFont>,
    pub content: ElementContent,
    pub presentation: Option<Presentation>,
    pub line_height: Option<f64>,
    pub max_width: Option<Dimension>,
    pub min_width: Option<Dimension>,
    pub min_height: Option<Dimension>,
    /// Text wider than the element is drawn to its edge and faded out
    /// over its last few characters, as Chrome's tab titles are
    /// (gfx::FADE_TAIL); the font's expected character width in pixels
    /// (`gfx::PlatformFont::GetExpectedTextWidth(1)`), which sets how long
    /// the fade is
    pub fade_tail: Option<f32>,
}

impl Element {
    pub fn new(font: &Rc<LoadedFont>, content: ElementContent) -> Self {
        Self {
            item_type: None,
            zindex: 0,
            display: DisplayType::Inline,
            float: Float::None,
            padding: BoxDimension::default(),
            margin: BoxDimension::default(),
            border: BoxDimension::default(),
            border_corners: None,
            vertical_align: VerticalAlign::default(),
            colors: ElementColors::default(),
            hover_colors: None,
            font: Rc::clone(font),
            content,
            presentation: None,
            line_height: None,
            max_width: None,
            min_width: None,
            min_height: None,
            fade_tail: None,
        }
    }

    pub fn with_line(font: &Rc<LoadedFont>, line: &Line, palette: &ColorPalette) -> Self {
        let mut content: Vec<Element> = vec![];
        let mut prior_attr = None;

        for cluster in line.cluster(None) {
            // Clustering may introduce cluster boundaries when the text hasn't actually
            // changed style. Undo that here.
            // There's still an issue where the style does actually change and we
            // subsequently don't clip the element.
            // <https://github.com/wezterm/wezterm/issues/2560>
            if let Some(prior) = content.last_mut() {
                let (fg, bg) = prior_attr.as_ref().unwrap();
                if cluster.attrs.background() == *bg && cluster.attrs.foreground() == *fg {
                    if let ElementContent::Text(t) = &mut prior.content {
                        t.push_str(&cluster.text);
                        continue;
                    }
                }
            }

            let child =
                Element::new(font, ElementContent::Text(cluster.text)).colors(ElementColors {
                    border: BorderColor::default(),
                    bg: if cluster.attrs.background() == ColorAttribute::Default {
                        InheritableColor::Inherited
                    } else {
                        palette
                            .resolve_bg(cluster.attrs.background())
                            .to_linear()
                            .into()
                    },
                    text: if cluster.attrs.foreground() == ColorAttribute::Default {
                        InheritableColor::Inherited
                    } else {
                        palette
                            .resolve_fg(cluster.attrs.foreground())
                            .to_linear()
                            .into()
                    },
                });

            content.push(child);
            prior_attr.replace((cluster.attrs.foreground(), cluster.attrs.background()));
        }

        Self::new(font, ElementContent::Children(content))
    }

    pub fn vertical_align(mut self, align: VerticalAlign) -> Self {
        self.vertical_align = align;
        self
    }

    pub fn item_type(mut self, item_type: UIItemType) -> Self {
        self.item_type.replace(item_type);
        self
    }

    pub fn display(mut self, display: DisplayType) -> Self {
        self.display = display;
        self
    }

    pub fn float(mut self, float: Float) -> Self {
        self.float = float;
        self
    }

    pub fn colors(mut self, colors: ElementColors) -> Self {
        self.colors = colors;
        self
    }

    pub fn hover_colors(mut self, colors: Option<ElementColors>) -> Self {
        self.hover_colors = colors;
        self
    }

    pub fn line_height(mut self, line_height: Option<f64>) -> Self {
        self.line_height = line_height;
        self
    }

    pub fn zindex(mut self, zindex: i8) -> Self {
        self.zindex = zindex;
        self
    }

    pub fn padding(mut self, padding: BoxDimension) -> Self {
        self.padding = padding;
        self
    }

    pub fn border(mut self, border: BoxDimension) -> Self {
        self.border = border;
        self
    }

    pub fn border_corners(mut self, corners: Option<Corners>) -> Self {
        self.border_corners = corners;
        self
    }

    pub fn margin(mut self, margin: BoxDimension) -> Self {
        self.margin = margin;
        self
    }

    pub fn max_width(mut self, width: Option<Dimension>) -> Self {
        self.max_width = width;
        self
    }

    pub fn min_width(mut self, width: Option<Dimension>) -> Self {
        self.min_width = width;
        self
    }

    pub fn min_height(mut self, height: Option<Dimension>) -> Self {
        self.min_height = height;
        self
    }

    /// See `fade_tail`
    pub fn fade_tail(mut self, expected_char_width: Option<f32>) -> Self {
        self.fade_tail = expected_char_width;
        self
    }
}

#[derive(Debug, Clone)]
pub enum ElementContent {
    Text(String),
    Children(Vec<Element>),
    Poly {
        line_width: isize,
        poly: SizedPoly,
    },
    /// An SVG file drawn as a mask in the text colour, `size` square
    Icon {
        path: String,
        size: Dimension,
    },
    /// A picture file (PNG) drawn as it is: `hover` under the pointer,
    /// `backdrop` while the window lacks the focus
    Image {
        normal: String,
        hover: Option<String>,
        backdrop: Option<String>,
        width: Dimension,
        height: Dimension,
    },
}

/// The width a fading element's children are laid out in: all of their
/// text, which is cut at the element's edge when drawn
const OVERFLOW_WIDTH: f32 = 1_000_000.;

pub struct LayoutContext<'a> {
    pub width: DimensionContext,
    pub height: DimensionContext,
    pub bounds: RectF,
    pub metrics: &'a RenderMetrics,
    pub gl_state: &'a RenderState,
    pub zindex: i8,
}

#[derive(Debug, Clone)]
pub struct ComputedElement {
    pub item_type: Option<UIItemType>,
    pub zindex: i8,
    /// The outer bounds of the element box (its margin)
    pub bounds: RectF,
    /// The outer bounds of the area enclosed by its border
    pub border_rect: RectF,
    pub border: PixelDimension,
    pub border_corners: Option<PixelCorners>,
    pub colors: ElementColors,
    pub hover_colors: Option<ElementColors>,
    /// The outer bounds of the area enclosed by the padding
    pub padding: RectF,
    /// The outer bounds of the content
    pub content_rect: RectF,
    pub baseline: f32,
    /// Its text overflows it and fades out at its end (`Element::fade_tail`)
    pub fade: Option<Fade>,

    pub content: ComputedElementContent,
}

/// How text that overflows an element fades out towards its right edge
/// (gfx::RenderText::ApplyFadeEffects, CreateFadeShader): linearly over
/// `width` pixels, down to `end_alpha`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub width: f32,
    pub end_alpha: f32,
}

impl Fade {
    /// The fade of text in a `display_width`-wide rect, for a font of
    /// `char_width` expected character width: over about three characters
    /// (a third of the width, when very narrow), to transparent, but when
    /// fewer than four characters fit, to up to 20 % (51 of 255) at no
    /// width, so the last faded characters stay a little legible
    /// (`CalculateFadeGradientWidth`, `CreateFadeShader`)
    pub fn new(char_width: f32, display_width: f32) -> Option<Self> {
        // GetExpectedTextWidth(n): round(n * average) with Skia's fonts,
        // ceil on the Mac
        let expected = |n: f32| {
            if cfg!(target_os = "macos") {
                (n * char_width).ceil()
            } else {
                (n * char_width).round()
            }
        };
        let width = expected(3.).min((display_width / 3.).round());
        if width <= 0. {
            return None;
        }
        let four = expected(4.);
        let fraction = if four > 0. { display_width / four } else { 1. };
        let end_alpha = if fraction < 1. {
            ((1. - fraction) * 51.).round() / 255.
        } else {
            0.
        };
        Some(Self { width, end_alpha })
    }
}

/// A fade for the text of an element's descendants: `Fade` ending at the
/// element's content's right edge `right`, where the text is cut.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FadeClip {
    right: f32,
    fade: Fade,
}

impl FadeClip {
    /// The text's opacity at `x`
    fn alpha(&self, x: f32) -> f32 {
        let start = self.right - self.fade.width;
        if x <= start || self.fade.width <= 0. {
            1.
        } else {
            let t = ((x - start) / self.fade.width).min(1.);
            1. + (self.fade.end_alpha - 1.) * t
        }
    }
}

impl ComputedElement {
    pub fn translate(&mut self, delta: euclid::Vector2D<f32, PixelUnit>) {
        self.bounds = self.bounds.translate(delta);
        self.border_rect = self.border_rect.translate(delta);
        self.padding = self.padding.translate(delta);
        self.content_rect = self.content_rect.translate(delta);

        match &mut self.content {
            ComputedElementContent::Children(kids) => {
                for kid in kids {
                    kid.translate(delta)
                }
            }
            ComputedElementContent::Text(_) => {}
            ComputedElementContent::Poly { .. }
            | ComputedElementContent::Icon { .. }
            | ComputedElementContent::Image { .. } => {}
        }
    }

    pub fn ui_items(&self) -> Vec<UIItem> {
        let mut items = vec![];
        self.ui_item_impl(&mut items);
        items
    }

    fn ui_item_impl(&self, items: &mut Vec<UIItem>) {
        if let Some(item_type) = &self.item_type {
            items.push(UIItem {
                x: self.bounds.min_x().max(0.) as usize,
                y: self.bounds.min_y().max(0.) as usize,
                width: self.bounds.width().max(0.) as usize,
                height: self.bounds.height().max(0.) as usize,
                item_type: item_type.clone(),
            });
        }

        match &self.content {
            ComputedElementContent::Text(_) => {}
            ComputedElementContent::Children(kids) => {
                for kid in kids {
                    kid.ui_item_impl(items);
                }
            }
            ComputedElementContent::Poly { .. }
            | ComputedElementContent::Icon { .. }
            | ComputedElementContent::Image { .. } => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum ComputedElementContent {
    Text(Vec<ElementCell>),
    Children(Vec<ComputedElement>),
    Poly {
        line_width: isize,
        poly: PixelSizedPoly,
    },
    Icon {
        path: String,
        size: f32,
    },
    Image {
        normal: String,
        hover: Option<String>,
        backdrop: Option<String>,
        width: f32,
        height: f32,
    },
}

#[derive(Debug, Clone)]
pub enum ElementCell {
    Sprite(Sprite),
    Glyph(Rc<CachedGlyph>),
}

#[derive(Debug)]
struct Rects {
    padding: RectF,
    border_rect: RectF,
    bounds: RectF,
    content_rect: RectF,
    translate: euclid::Vector2D<f32, PixelUnit>,
}

impl Element {
    fn compute_rects(&self, context: &LayoutContext, content_rect: RectF) -> Rects {
        let padding = self.padding.to_pixels(context);
        let margin = self.margin.to_pixels(context);
        let border = self.border.to_pixels(context);

        let padding = euclid::rect(
            content_rect.min_x() - padding.left,
            content_rect.min_y() - padding.top,
            content_rect.width() + padding.left + padding.right,
            content_rect.height() + padding.top + padding.bottom,
        );

        let border_rect = euclid::rect(
            padding.min_x() - border.left,
            padding.min_y() - border.top,
            padding.width() + border.left + border.right,
            padding.height() + border.top + border.bottom,
        );

        let bounds = euclid::rect(
            border_rect.min_x() - margin.left,
            border_rect.min_y() - margin.top,
            border_rect.width() + margin.left + margin.right,
            border_rect.height() + margin.top + margin.bottom,
        );
        let translate = euclid::vec2(
            context.bounds.min_x() - bounds.min_x(),
            context.bounds.min_y() - bounds.min_y(),
        );
        Rects {
            padding: padding.translate(translate),
            border_rect: border_rect.translate(translate),
            bounds: bounds.translate(translate),
            content_rect: content_rect.translate(translate),
            translate,
        }
    }
}

impl super::TermWindow {
    pub fn compute_element<'a>(
        &self,
        context: &LayoutContext,
        element: &Element,
    ) -> anyhow::Result<ComputedElement> {
        let local_metrics;
        let local_context;
        let context = if let Some(line_height) = element.line_height {
            local_metrics = context.metrics.scale_line_height(line_height);
            local_context = LayoutContext {
                height: DimensionContext {
                    dpi: context.height.dpi,
                    pixel_max: context.height.pixel_max,
                    pixel_cell: context.height.pixel_cell * line_height as f32,
                },
                width: context.width,
                bounds: context.bounds,
                gl_state: context.gl_state,
                metrics: &local_metrics,
                zindex: context.zindex,
            };
            &local_context
        } else {
            context
        };
        let border_corners = element
            .border_corners
            .as_ref()
            .map(|c| c.to_pixels(context));
        let style = element.font.style();
        let border = element.border.to_pixels(context);
        let padding = element.padding.to_pixels(context);
        let baseline = context.height.pixel_cell + context.metrics.descender.get() as f32;
        let min_width = match element.min_width {
            Some(w) => w.evaluate_as_pixels(context.width),
            None => 0.0,
        };
        let min_height = match element.min_height {
            Some(h) => h.evaluate_as_pixels(context.height),
            None => 0.0,
        };

        let border_and_padding_width = border.left + border.right + padding.left + padding.right;

        let max_width = match element.max_width {
            Some(w) => {
                w.evaluate_as_pixels(context.width)
                    .min(context.bounds.width())
                    - border_and_padding_width
            }
            None => context.bounds.width() - border_and_padding_width,
        }
        .min((context.width.pixel_max - context.bounds.min_x()) - border_and_padding_width);

        match &element.content {
            ElementContent::Text(s) => {
                let window = self.window.as_ref().unwrap().clone();
                let direction = wezterm_bidi::Direction::LeftToRight;
                let infos = element.font.shape(
                    &s,
                    move || window.notify(TermWindowNotif::InvalidateShapeCache),
                    BlockKey::filter_out_synthetic,
                    element.presentation,
                    direction,
                    None,
                    None,
                )?;
                let mut computed_cells = vec![];
                let mut glyph_cache = context.gl_state.glyph_cache.borrow_mut();
                let mut pixel_width = 0.0;
                let mut x_pos = context.bounds.min_x();
                let mut min_y = 0.0f32;
                let max_x = context.bounds.min_x() + max_width;

                for info in infos {
                    let cell_start = &s[info.cluster as usize..];
                    let mut iter = Graphemes::new(cell_start).peekable();
                    let grapheme = iter
                        .next()
                        .ok_or_else(|| anyhow!("info.cluster didn't map into string"))?;
                    if let Some(key) = BlockKey::from_str(grapheme) {
                        if pixel_width + context.width.pixel_cell >= max_x {
                            break;
                        }
                        pixel_width += context.width.pixel_cell;
                        x_pos += context.width.pixel_cell;
                        let sprite = glyph_cache.cached_block(key, context.metrics)?;
                        computed_cells.push(ElementCell::Sprite(sprite));
                    } else {
                        let next_grapheme: Option<&str> = iter.peek().map(|s| *s);
                        let followed_by_space = next_grapheme == Some(" ");
                        let num_cells = grapheme_column_width(grapheme, None);
                        let glyph = glyph_cache.cached_glyph(
                            &info,
                            style,
                            followed_by_space,
                            &element.font,
                            context.metrics,
                            num_cells as u8,
                        )?;

                        if let Some(texture) = glyph.texture.as_ref() {
                            let x_pos = x_pos + (glyph.x_offset + glyph.bearing_x).get() as f32;
                            let width = texture.coords.size.width as f32 * glyph.scale as f32;
                            if x_pos + width >= max_x {
                                break;
                            }
                        } else if x_pos + glyph.x_advance.get() as f32 >= max_x {
                            break;
                        }

                        min_y =
                            min_y.min(baseline - (glyph.y_offset + glyph.bearing_y).get() as f32);

                        pixel_width += glyph.x_advance.get() as f32;
                        x_pos += glyph.x_advance.get() as f32;

                        computed_cells.push(ElementCell::Glyph(glyph));
                    }
                }

                let content_rect = euclid::rect(
                    0.,
                    0.,
                    pixel_width.max(min_width),
                    context.height.pixel_cell.max(min_height),
                );

                let rects = element.compute_rects(context, content_rect);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    fade: None,
                    content: ComputedElementContent::Text(computed_cells),
                })
            }
            ElementContent::Children(kids) => {
                // a fading element's text runs on past its edge, to be
                // cut there when drawn (`render_text_quad`)
                let overflow = element.fade_tail.is_some();
                let mut block_pixel_width: f32 = 0.;
                let mut block_pixel_height: f32 = 0.;
                let mut computed_kids = vec![];
                let mut max_x: f32 = 0.;
                let mut float_width: f32 = 0.;
                let mut y_coord: f32 = 0.;

                for child in kids {
                    if child.display == DisplayType::Block {
                        y_coord += block_pixel_height;
                        block_pixel_height = 0.;
                        block_pixel_width = 0.;
                    }

                    let bounds = match child.float {
                        Float::None if overflow => euclid::rect(
                            block_pixel_width,
                            y_coord,
                            OVERFLOW_WIDTH,
                            context.bounds.max_y() - (context.bounds.min_y() + y_coord),
                        ),
                        Float::None => euclid::rect(
                            block_pixel_width,
                            y_coord,
                            context.bounds.max_x() - (context.bounds.min_x() + block_pixel_width),
                            context.bounds.max_y() - (context.bounds.min_y() + y_coord),
                        ),
                        Float::Right => euclid::rect(
                            0.,
                            y_coord,
                            context.bounds.width(),
                            context.bounds.max_y() - (context.bounds.min_y() + y_coord),
                        ),
                    };
                    let kid = self.compute_element(
                        &LayoutContext {
                            bounds,
                            gl_state: context.gl_state,
                            height: context.height,
                            metrics: context.metrics,
                            width: DimensionContext {
                                dpi: context.width.dpi,
                                pixel_cell: context.width.pixel_cell,
                                pixel_max: if overflow { OVERFLOW_WIDTH } else { max_width },
                            },
                            zindex: context.zindex + element.zindex,
                        },
                        child,
                    )?;
                    match child.float {
                        Float::Right => {
                            float_width += float_width.max(kid.bounds.width());
                        }
                        Float::None => {
                            block_pixel_width += kid.bounds.width();
                            max_x = max_x.max(block_pixel_width);
                        }
                    }
                    block_pixel_height = block_pixel_height.max(kid.bounds.height());

                    computed_kids.push(kid);
                }

                // Respect min-width
                max_x = max_x.max(min_width);

                let mut float_max_x = (max_x + float_width).min(max_width);

                let pixel_height = (y_coord + block_pixel_height).max(min_height);

                for (kid, child) in computed_kids.iter_mut().zip(kids.iter()) {
                    match child.float {
                        Float::Right => {
                            max_x = max_x.max(float_max_x);
                            let x = float_max_x - kid.bounds.width();
                            float_max_x -= kid.bounds.width();
                            kid.translate(euclid::vec2(x, 0.));
                        }
                        _ => {}
                    }
                    match child.vertical_align {
                        VerticalAlign::Bottom => {
                            kid.translate(euclid::vec2(0., pixel_height - kid.bounds.height()));
                        }
                        VerticalAlign::Middle => {
                            // on a whole pixel, so text and pictures are
                            // not resampled across rows
                            kid.translate(euclid::vec2(
                                0.,
                                ((pixel_height - kid.bounds.height()) / 2.0).round(),
                            ));
                        }
                        VerticalAlign::Top => {}
                    }
                }

                computed_kids.sort_by(|a, b| a.zindex.cmp(&b.zindex));

                // overflowing text is always cut at the edge; too narrow a
                // room for a fade (a third of it rounds to nothing), it is
                // cut without one, and none shows in no room at all, as
                // Chrome's title label is drawn only within its bounds
                let fade =
                    element
                        .fade_tail
                        .filter(|_| max_x > max_width + 0.5)
                        .map(|char_width| {
                            Fade::new(char_width, max_width).unwrap_or(Fade {
                                width: 0.,
                                end_alpha: 1.,
                            })
                        });
                let content_rect = euclid::rect(0., 0., max_x.min(max_width), pixel_height);
                let rects = element.compute_rects(context, content_rect);

                for kid in &mut computed_kids {
                    kid.translate(rects.translate);
                }

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    fade,
                    content: ComputedElementContent::Children(computed_kids),
                })
            }
            ElementContent::Poly { poly, line_width } => {
                let poly = poly.to_pixels(context);
                let content_rect = euclid::rect(0., 0., poly.width, poly.height.max(min_height));
                let rects = element.compute_rects(context, content_rect);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    fade: None,
                    content: ComputedElementContent::Poly {
                        poly,
                        line_width: *line_width,
                    },
                })
            }
            ElementContent::Icon { path, size } => {
                let size = size.evaluate_as_pixels(context.width).round();
                let content_rect = euclid::rect(0., 0., size, size.max(min_height));
                let rects = element.compute_rects(context, content_rect);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    fade: None,
                    content: ComputedElementContent::Icon {
                        path: path.clone(),
                        size,
                    },
                })
            }
            ElementContent::Image {
                normal,
                hover,
                backdrop,
                width,
                height,
            } => {
                let width = width.evaluate_as_pixels(context.width).round();
                let height = height.evaluate_as_pixels(context.height).round();
                let content_rect = euclid::rect(0., 0., width, height.max(min_height));
                let rects = element.compute_rects(context, content_rect);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    fade: None,
                    content: ComputedElementContent::Image {
                        normal: normal.clone(),
                        hover: hover.clone(),
                        backdrop: backdrop.clone(),
                        width,
                        height,
                    },
                })
            }
        }
    }

    /// Renders `element` and its descendants. The tree is walked once to
    /// list its elements in order, then drawn a layer (zindex) at a time,
    /// each layer's vertex buffers mapped once: a map waits for the GPU
    /// to be done with them (milliseconds with OpenGL), and mapping them
    /// for each element in turn made a tab bar with many tabs cost tens
    /// of milliseconds a frame. Within a layer the elements keep their
    /// order; the layers are drawn in zindex order whatever the order
    /// their quads were made in.
    pub fn render_element<'a>(
        &self,
        element: &ComputedElement,
        gl_state: &RenderState,
        inherited_colors: Option<&ElementColors>,
    ) -> anyhow::Result<()> {
        self.render_element_with_opacity(element, gl_state, inherited_colors, 1.)
    }

    /// `render_element`, the whole of it at `opacity` (a card fading in
    /// or out)
    pub fn render_element_with_opacity(
        &self,
        element: &ComputedElement,
        gl_state: &RenderState,
        inherited_colors: Option<&ElementColors>,
        opacity: f32,
    ) -> anyhow::Result<()> {
        let mut flat = vec![];
        self.flatten_element(element, inherited_colors.cloned(), None, &mut flat);
        let mut zindexes: Vec<i8> = flat.iter().map(|(e, _, _)| e.zindex).collect();
        zindexes.sort_unstable();
        zindexes.dedup();
        for zindex in zindexes {
            let layer = gl_state.layer_for_zindex(zindex)?;
            let mut layers = layer.quad_allocator();
            let marks = layers.marks();
            for (e, inherited, clip) in flat.iter().filter(|(e, _, _)| e.zindex == zindex) {
                self.render_one_element(e, &mut layers, inherited.as_ref(), *clip)?;
            }
            if opacity < 1. {
                layers.fade_from(marks, opacity.max(0.));
            }
        }
        Ok(())
    }

    /// `element` and its descendants in drawing order, each with the
    /// colours it inherits and the fade its text is cut by, if any.
    fn flatten_element<'e>(
        &self,
        element: &'e ComputedElement,
        inherited: Option<ElementColors>,
        clip: Option<FadeClip>,
        out: &mut Vec<(&'e ComputedElement, Option<ElementColors>, Option<FadeClip>)>,
    ) {
        if let ComputedElementContent::Children(kids) = &element.content {
            let effective = self
                .element_colors(element)
                .inherit_from(inherited.as_ref());
            let kids_clip = element
                .fade
                .map(|fade| FadeClip {
                    right: element.content_rect.max_x(),
                    fade,
                })
                .or(clip);
            out.push((element, inherited, clip));
            for kid in kids {
                self.flatten_element(kid, Some(effective.clone()), kids_clip, out);
            }
        } else {
            out.push((element, inherited, clip));
        }
    }

    /// The colours `element` shows: its hover colours under the pointer.
    fn element_colors<'e>(&self, element: &'e ComputedElement) -> &'e ElementColors {
        match &element.hover_colors {
            Some(hc) if self.is_hovering(element) => hc,
            _ => &element.colors,
        }
    }

    /// Whether the pointer is over `element` (and not captured elsewhere).
    fn is_hovering(&self, element: &ComputedElement) -> bool {
        let over = match &self.current_mouse_event {
            Some(event) => {
                let mouse_x = event.coords.x as f32;
                let mouse_y = event.coords.y as f32;
                mouse_x >= element.bounds.min_x()
                    && mouse_x <= element.bounds.max_x()
                    && mouse_y >= element.bounds.min_y()
                    && mouse_y <= element.bounds.max_y()
            }
            None => false,
        };
        over && matches!(self.current_mouse_capture, None | Some(MouseCapture::UI))
    }

    /// One element's own quads (its children are drawn in their turn).
    fn render_one_element(
        &self,
        element: &ComputedElement,
        layers: &mut TripleLayerQuadAllocator,
        inherited_colors: Option<&ElementColors>,
        clip: Option<FadeClip>,
    ) -> anyhow::Result<()> {
        let colors = self.element_colors(element);

        self.render_element_background(element, colors, layers, inherited_colors)?;
        let left = self.dimensions.pixel_width as f32 / -2.0;
        let top = self.dimensions.pixel_height as f32 / -2.0;
        match &element.content {
            ComputedElementContent::Text(cells) => {
                let text = self.resolve_text(colors, inherited_colors);
                // where the text stops: the element's end, or, overflowing
                // a fading element, that element's edge
                let max_x = match clip {
                    Some(clip) => clip.right,
                    None => element.content_rect.max_x(),
                };
                let mut pos_x = element.content_rect.min_x();
                for cell in cells {
                    if pos_x >= max_x {
                        break;
                    }
                    match cell {
                        ElementCell::Sprite(sprite) => {
                            let width = sprite.coords.width() as f32;
                            let height = sprite.coords.height() as f32;
                            let pos_y = top + element.content_rect.min_y();

                            if clip.is_none() && pos_x + width > max_x {
                                break;
                            }
                            self.render_text_quad(
                                layers,
                                2,
                                (pos_x + left, pos_x + left + width),
                                (pos_y, pos_y + height),
                                sprite.texture_coords(),
                                false,
                                &text,
                                clip.map(|c| FadeClip {
                                    right: c.right + left,
                                    ..c
                                }),
                            )?;
                            pos_x += width;
                        }
                        ElementCell::Glyph(glyph) => {
                            if let Some(texture) = glyph.texture.as_ref() {
                                let pos_y = element.content_rect.min_y() as f32 + top
                                    - (glyph.y_offset + glyph.bearing_y).get() as f32
                                    + element.baseline;

                                if clip.is_none() && pos_x + glyph.x_advance.get() as f32 > max_x {
                                    break;
                                }
                                let pos_x = pos_x + (glyph.x_offset + glyph.bearing_x).get() as f32;
                                let width = texture.coords.size.width as f32 * glyph.scale as f32;
                                let height = texture.coords.size.height as f32 * glyph.scale as f32;

                                self.render_text_quad(
                                    layers,
                                    1,
                                    (pos_x + left, pos_x + left + width),
                                    (pos_y, pos_y + height),
                                    texture.texture_coords(),
                                    glyph.has_color,
                                    &text,
                                    clip.map(|c| FadeClip {
                                        right: c.right + left,
                                        ..c
                                    }),
                                )?;
                            }
                            pos_x += glyph.x_advance.get() as f32;
                        }
                    }
                }
            }
            // (drawn in their own turn, see `render_element`)
            ComputedElementContent::Children(_) => {}
            ComputedElementContent::Poly { poly, line_width } => {
                if element.content_rect.width() >= poly.width {
                    let mut quad = self.poly_quad(
                        layers,
                        1,
                        element.content_rect.origin,
                        poly.poly,
                        *line_width,
                        euclid::size2(poly.width, poly.height),
                        LinearRgba::TRANSPARENT,
                    )?;
                    self.resolve_text(colors, inherited_colors).apply(&mut quad);
                }
            }
            ComputedElementContent::Icon { path, size } => {
                // centred in the content box, whose height the row may
                // have stretched
                let top = element.content_rect.min_y()
                    + ((element.content_rect.height() - size) / 2.).max(0.);
                let origin = euclid::point2(element.content_rect.min_x(), top.round());
                if let Some(mut quad) = self.icon_quad(layers, 1, origin, path, *size)? {
                    self.resolve_text(colors, inherited_colors).apply(&mut quad);
                }
            }
            ComputedElementContent::Image {
                normal,
                hover,
                backdrop,
                width,
                height,
            } => {
                let path = if self.is_hovering(element) {
                    hover.as_ref().unwrap_or(normal)
                } else if self.focused.is_none() {
                    backdrop.as_ref().unwrap_or(normal)
                } else {
                    normal
                };
                let top = element.content_rect.min_y()
                    + ((element.content_rect.height() - height) / 2.).max(0.);
                let origin = euclid::point2(element.content_rect.min_x(), top.round());
                self.image_quad(layers, 1, origin, path, *width, *height)?;
            }
        }

        Ok(())
    }

    /// A glyph's (or a sprite's) quad at `x` by `y` (window coordinates,
    /// centred), cut at `clip`'s edge and faded towards it as Chrome fades
    /// a tab title. The fade is linear in x, so a quad is split where it
    /// begins and each piece's ends get the fade's opacity at them; a faded
    /// piece is drawn as a grayscale mask, whose shader multiplies the
    /// colour's alpha in (the glyph mode takes the alpha from the texture
    /// alone). A colour emoji is cut but not faded.
    #[allow(clippy::too_many_arguments)]
    fn render_text_quad(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        layer: usize,
        x: (f32, f32),
        y: (f32, f32),
        tex: TextureRect,
        has_color: bool,
        color: &ResolvedColor,
        clip: Option<FadeClip>,
    ) -> anyhow::Result<()> {
        let (x0, x1) = x;
        let (u0, u1, v0, v1) = (tex.min_x(), tex.max_x(), tex.min_y(), tex.max_y());
        let u_at = |x: f32| {
            if x1 > x0 {
                u0 + (u1 - u0) * (x - x0) / (x1 - x0)
            } else {
                u0
            }
        };
        let mut edges = [x0, x1, x1];
        let mut pieces = 1;
        if let Some(clip) = clip {
            if x0 >= clip.right {
                return Ok(());
            }
            let end = x1.min(clip.right);
            let start = clip.right - clip.fade.width;
            if start > x0 && start < end {
                edges = [x0, start, end];
                pieces = 2;
            } else {
                edges = [x0, end, end];
            }
        }
        for i in 0..pieces {
            let (a, b) = (edges[i], edges[i + 1]);
            if b <= a {
                continue;
            }
            let mut quad = layers.allocate(layer)?;
            quad.set_position(a, y.0, b, y.1);
            color.apply(&mut quad);
            quad.set_texture_discrete(u_at(a), u_at(b), v0, v1);
            quad.set_hsv(None);
            let (alpha_a, alpha_b) = clip.map_or((1., 1.), |c| (c.alpha(a), c.alpha(b)));
            if !has_color && (alpha_a < 1. || alpha_b < 1.) {
                quad.set_grayscale();
                quad.fade(alpha_a, alpha_b);
            } else {
                quad.set_has_color(has_color);
            }
        }
        Ok(())
    }

    fn resolve_text(
        &self,
        colors: &ElementColors,
        inherited_colors: Option<&ElementColors>,
    ) -> ResolvedColor {
        match &colors.text {
            InheritableColor::Inherited => match inherited_colors {
                Some(colors) => self.resolve_text(colors, None),
                None => LinearRgba::TRANSPARENT.into(),
            },
            InheritableColor::Color(color) => (*color).into(),
            InheritableColor::Animated {
                color,
                alt_color,
                ease,
                one_shot,
            } => {
                if let Some((mix_value, next)) = ease.borrow_mut().intensity(*one_shot) {
                    self.update_next_frame_time(Some(next));
                    ResolvedColor {
                        color: *color,
                        alt_color: *alt_color,
                        mix_value,
                    }
                } else {
                    (*color).into()
                }
            }
        }
    }

    fn resolve_bg(
        &self,
        colors: &ElementColors,
        inherited_colors: Option<&ElementColors>,
    ) -> ResolvedColor {
        match &colors.bg {
            InheritableColor::Inherited => match inherited_colors {
                Some(colors) => self.resolve_bg(colors, None),
                None => LinearRgba::TRANSPARENT.into(),
            },
            InheritableColor::Color(color) => (*color).into(),
            InheritableColor::Animated {
                color,
                alt_color,
                ease,
                one_shot,
            } => {
                if let Some((mix_value, next)) = ease.borrow_mut().intensity(*one_shot) {
                    self.update_next_frame_time(Some(next));
                    ResolvedColor {
                        color: *color,
                        alt_color: *alt_color,
                        mix_value,
                    }
                } else {
                    (*color).into()
                }
            }
        }
    }

    fn render_element_background<'a>(
        &self,
        element: &ComputedElement,
        colors: &ElementColors,
        layers: &mut TripleLayerQuadAllocator,
        inherited_colors: Option<&ElementColors>,
    ) -> anyhow::Result<()> {
        let mut top_left_width = 0.;
        let mut top_left_height = 0.;
        let mut top_right_width = 0.;
        let mut top_right_height = 0.;

        let mut bottom_left_width = 0.;
        let mut bottom_left_height = 0.;
        let mut bottom_right_width = 0.;
        let mut bottom_right_height = 0.;

        if let Some(c) = &element.border_corners {
            top_left_width = c.top_left.width;
            top_left_height = c.top_left.height;
            top_right_width = c.top_right.width;
            top_right_height = c.top_right.height;

            bottom_left_width = c.bottom_left.width;
            bottom_left_height = c.bottom_left.height;
            bottom_right_width = c.bottom_right.width;
            bottom_right_height = c.bottom_right.height;

            if top_left_width > 0. && top_left_height > 0. {
                self.poly_quad(
                    layers,
                    0,
                    element.border_rect.origin,
                    c.top_left.poly,
                    element.border.top as isize,
                    euclid::size2(top_left_width, top_left_height),
                    colors.border.top,
                )?
                .set_grayscale();
            }
            if top_right_width > 0. && top_right_height > 0. {
                self.poly_quad(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.max_x() - top_right_width,
                        element.border_rect.min_y(),
                    ),
                    c.top_right.poly,
                    element.border.top as isize,
                    euclid::size2(top_right_width, top_right_height),
                    colors.border.top,
                )?
                .set_grayscale();
            }
            if bottom_left_width > 0. && bottom_left_height > 0. {
                self.poly_quad(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.min_x(),
                        element.border_rect.max_y() - bottom_left_height,
                    ),
                    c.bottom_left.poly,
                    element.border.bottom as isize,
                    euclid::size2(bottom_left_width, bottom_left_height),
                    colors.border.bottom,
                )?
                .set_grayscale();
            }
            if bottom_right_width > 0. && bottom_right_height > 0. {
                self.poly_quad(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.max_x() - bottom_right_width,
                        element.border_rect.max_y() - bottom_right_height,
                    ),
                    c.bottom_right.poly,
                    element.border.bottom as isize,
                    euclid::size2(bottom_right_width, bottom_right_height),
                    colors.border.bottom,
                )?
                .set_grayscale();
            }

            // Filling the background is more complex because we can't
            // simply fill the padding rect--we'd clobber the corner
            // graphics.
            // Instead, we consider the element as consisting of:
            //
            //   TL T TR
            //   L  C  R
            //   BL B BR
            //
            // We already rendered the corner pieces, so now we need
            // to do the rest

            // The `T` piece
            let mut quad = self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width,
                    element.border_rect.min_y(),
                    element.border_rect.width() - (top_left_width + top_right_width) as f32,
                    top_left_height.max(top_right_height),
                ),
                LinearRgba::TRANSPARENT,
            )?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);

            // The `B` piece
            let mut quad = self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + bottom_left_width,
                    element.border_rect.max_y() - bottom_left_height.max(bottom_right_height),
                    element.border_rect.width() - (bottom_left_width + bottom_right_width),
                    bottom_left_height.max(bottom_right_height),
                ),
                LinearRgba::TRANSPARENT,
            )?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);

            // The `L` piece
            let mut quad = self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x(),
                    element.border_rect.min_y() + top_left_height,
                    top_left_width.max(bottom_left_width),
                    element.border_rect.height() - (top_left_height + bottom_left_height),
                ),
                LinearRgba::TRANSPARENT,
            )?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);

            // The `R` piece
            let mut quad = self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.max_x() - top_right_width.max(bottom_right_width),
                    element.border_rect.min_y() + top_right_height,
                    top_right_width.max(bottom_right_width),
                    element.border_rect.height() - (top_right_height + bottom_right_height),
                ),
                LinearRgba::TRANSPARENT,
            )?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);

            // The `C` piece
            let mut quad = self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width,
                    element.border_rect.min_y() + top_right_height.min(top_left_height),
                    element.border_rect.width() - (top_left_width + top_right_width),
                    element.border_rect.height()
                        - (top_right_height.min(top_left_height)
                            + bottom_right_height.min(bottom_left_height)),
                ),
                LinearRgba::TRANSPARENT,
            )?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);
        } else if colors.bg != InheritableColor::Color(LinearRgba::TRANSPARENT) {
            let mut quad =
                self.filled_rectangle(layers, 0, element.padding, LinearRgba::TRANSPARENT)?;
            self.resolve_bg(colors, inherited_colors).apply(&mut quad);
        }

        if element.border_rect == element.padding {
            // There's no border to be drawn
            return Ok(());
        }

        if element.border.top > 0. && colors.border.top != LinearRgba::TRANSPARENT {
            self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width as f32,
                    element.border_rect.min_y(),
                    element.border_rect.width() - (top_left_width + top_right_width) as f32,
                    element.border.top,
                ),
                colors.border.top,
            )?;
        }
        if element.border.bottom > 0. && colors.border.bottom != LinearRgba::TRANSPARENT {
            self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + bottom_left_width as f32,
                    element.border_rect.max_y() - element.border.bottom,
                    element.border_rect.width() - (bottom_left_width + bottom_right_width) as f32,
                    element.border.bottom,
                ),
                colors.border.bottom,
            )?;
        }
        if element.border.left > 0. && colors.border.left != LinearRgba::TRANSPARENT {
            self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x(),
                    element.border_rect.min_y() + top_left_height as f32,
                    element.border.left,
                    element.border_rect.height() - (top_left_height + bottom_left_height) as f32,
                ),
                colors.border.left,
            )?;
        }
        if element.border.right > 0. && colors.border.right != LinearRgba::TRANSPARENT {
            self.filled_rectangle(
                layers,
                0,
                euclid::rect(
                    element.border_rect.max_x() - element.border.right,
                    element.border_rect.min_y() + top_right_height as f32,
                    element.border.left,
                    element.border_rect.height() - (top_right_height + bottom_right_height) as f32,
                ),
                colors.border.right,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gfx's fade: over three expected characters, a third of the width
    /// when narrow, to transparent, or (fewer than four characters wide)
    /// to up to 51/255 at no width
    #[test]
    fn fade_as_chrome() {
        // 8 px characters: 24 px of fade, to nothing
        assert_eq!(
            Fade::new(8., 200.),
            Some(Fade {
                width: 24.,
                end_alpha: 0.
            })
        );
        // 20 px wide: a third of it (7), and 20/32 of four characters,
        // so 51 * (1 - 0.625) = 19 of 255 left at the end
        assert_eq!(
            Fade::new(8., 20.),
            Some(Fade {
                width: 7.,
                end_alpha: 19. / 255.
            })
        );
        assert_eq!(Fade::new(8., 1.), None, "no room for a fade");
        let clip = FadeClip {
            right: 100.,
            fade: Fade {
                width: 20.,
                end_alpha: 0.,
            },
        };
        assert_eq!(clip.alpha(70.), 1.);
        assert_eq!(clip.alpha(80.), 1.);
        assert_eq!(clip.alpha(90.), 0.5);
        assert_eq!(clip.alpha(100.), 0.);
    }
}
