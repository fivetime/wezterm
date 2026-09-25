# The Chrome tab strip in NativeTerm's WezTerm: what matches, what does not

`chrome-strip-spec.md` is every parameter of Chromium's Linux tab strip
and frame, read out of its source. This is the ledger of what this fork
implements against it, and how that is checked.

## How it is checked

```
cargo test -p chrome-strip
```

The `chrome-strip` crate computes the strip in pixels from the inputs
alone (window width, scale, tabs, the theme's caption buttons, the title
font's metrics) and its tests assert Chrome's numbers: no window, no
desktop, no screenshots. The renderer (`wezterm-gui/src/termwindow/render/
fancy_tab_bar.rs`, `chrome_tabs.rs`) takes every size and position from
`chrome_strip::Layout`; what it adds is the drawing. The masks it draws
with (the tab shape, the pill, the stroke) are tested in `chrome_tabs.rs`
(`cargo test -p wezterm-gui --bin wezterm-gui chrome`, which needs a Linux
build host for wezterm-gui's dependencies).

## Matched (with the spec's parameter names)

Layout: `strip.tab_strip_height` 41 with `tabstrip_toolbar_overlap` 1
(the terminal begins at row 41, the toolbar's top line on row 40);
`strip.tab_strip_padding` 6; the 28-DIP highlight row; `tab.standard_width`
256, `tab.min_active_width` 56, `tab.min_inactive_width` 32,
`tab.overlap` 18, `layout.width_distribution_algorithm` (all tabs equal
above the crossing width, the active tab kept at 56 below it, floored
widths and the DIP left over to the first tabs); `tab.top_corner_radius`
10 with `path.top_corner_radius_for_width`; `tab.bottom_corner_radius` 12;
`tab.contents_insets` 20; `region.leading_margin_param` 12 and
`frame.exclusions_linux` (the strip past leading buttons by the larger
of 12 and their margin; the trailing buttons' bounds plus the caption
margin); `ntb.position` (6 past the last tab) and `ntb.button_size` 28
with its 16 icon and 14 radius; `px.scale_and_align_bounds` for the
body's pixel edges; the hit test's 3-DIP reach into the separators and
its rise to the top when maximized.

Contents: `favicon.size` 16 at the contents' corner (rows 12..28);
`tab.pre_title_padding` 8; `title.bounds` (8 before the close icon);
`title.cap_height_centring` (`RenderText::DetermineBaselineCenteringText`
over the 41-DIP tab; the font's whole height when it has no cap height);
`close_button.position` (icon 16 from the contents' right, button 28
around it, top 6); `close_button.show_rules` (the active tab always, an
inactive one with 68 of contents room, nothing under 32 wide);
`close_button.ink_drop_colours_opacities` (a circle of radius 8 around
the icon, 0.16 of the colour contrasting most with the tab);
`new_tab_button.hover_highlight` (white at 0.16 whatever the strip,
Chrome's own quirk).

Paint: `path.fill.geometry` (the active tab with its feet, over the
toolbar's row); `path.highlight.geometry` (the detached pill with the
width's radius, `int(top + 28s)` tall); `stroke.when_drawn_rule`
(contrast under 1.3 between the active tab and the active frame) and
`stroke.color_recipe.toolbar_top_separator.system_theme` (the tab's
colour blended towards Grey 900 to 2:1, else towards white);
`stroke.toolbar_top_separator_line` (the line across row 40, the active
tab over it); `path.fill.stroke_adjustment` (half a pixel in, radii half a
pixel smaller); `separator.size` 2x16 with `separator.corner_radius` 1 on
row 12, `separator.bounds` (4 DIP inside each tab's edge, one between two
tabs, one after the last), `separator.opacity_rules` (none beside an
active tab, fading with a neighbour's hover); `hover.animation.duration`
200 ms with `hover.animation.curve_show` ease-out and `curve_hide`
ease-in, reversed from where it is; `hover.color_recipe.system_theme`
(the desktop accent's tone from the configuration, Chrome's baseline
#A8C7FA / #004A77 without one) and `tab.bg_inactive_hover_frame_inactive`
(neutral 10 at 0x0F or neutral 99 at 0x1A over the strip in an unfocused
window); `algo.IsDark` (luminance under 0.2117), `algo.AlphaBlend`,
`algo.BlendForMinContrast`, `algo.GetContrastRatio`.

Caption buttons: `nav.redraw_scale`, `nav.button_margin`,
`nav.top_area_spacing` (the GTK picture with its CSS margins and the
header padding centred in the 40-DIP top area, shrunk to fit);
`layout.window_caption_spacing` (GTK's 6 between buttons only) and
`nav.header_spacing`; `layout.button_ordering` from
`gtk-decoration-layout`.

Frame (X11): `x11.gtk_frame_extents` ceiled to pixels;
`x11.input_region` (the insets less the 10-DIP band);
`x11.opaque_region` (the content below its round corners);
`frame.resize_border` 10 and `hit.resize_area_corner_size` 16;
`frame.tiled_effects` (one-way maximized: the band alone, no shadow, no
radius); `frame.border_insets_condensed` (nothing when maximized);
`gtk_frame.*` (the theme's decoration rendered and nine-sliced, its
thickness and radius measured; drawn at scale from a 2x picture rather
than re-rendered per scale); `gtk_frame.solid_frame` gate
(`_GTK_FRAME_EXTENTS` known to the window manager, a compositor
present, not Xfwm4).

Chrome's own frame (`integrated_window_edge = { chrome = true }`, what
NativeTerm asks for on a Qt desktop, where Chrome's toolkit gives it no
frame: `chrome_strip::frame`, tested by `cargo test -p chrome-strip
frame`): `frame.md_shadow_values` (elevation 16 focused, 2 not: the key
shadow offset by the elevation, blurred 4x, at 0x3d; the ambient one
blurred 2x at 0x1f), `frame.shadow_sigma` (`RadiusToSigma` of half the
blur), `frame.border_insets_shadow` TLBR(10, 16, 32, 16) from the
shadows' extents and the 10-DIP band, `frame.exterior_border` (1 px
black at 0x26 outside the content), `frame.corner_radius` 8
(`Emphasis::kHigh`; 0 tiled or without a compositor); and, where the
window manager does not take `_GTK_FRAME_EXTENTS` (deepin's KWin),
`frame.solid` (`ShouldDrawRestoredFrameShadow` false): no shadow, the
frame's colour 4 DIP either side and below (`kFrameBorderThickness`),
none above, the whole window one round-cornered outline with a 1-px
interior line at 0x26 in black or white (`PickContrastingColor`), the
line's top run across the strip's first row, the content's top 4 DIP
resizing (`kResizeTopBorderThickness`), no `_GTK_FRAME_EXTENTS`. The
X11 window picks the variant from the same facts Chrome does
(`_GTK_FRAME_EXTENTS` in `_NET_SUPPORTED`, not Xfwm4; `_NET_WM_CM_S*`
owned); Wayland always has the shadow.

Hover card: `hover_card.card.client_width_dip` 256,
`hover_card.corner_radius_dip` 8, `hover_card.text.margins` 12,
`hover_card.show_delay.algorithm` (300 ms up to a 64-wide tab, 800 at
256, 500 more there), `hover_card.placement.anchor_rect` (the tab's
own left edge, 2 DIP up from its bottom).

Text: `text.gtk_font_name_parse` (the desktop font at Chrome's
whole-pixel size, sent by NativeTerm), `text.font_metrics_linux`
(ceiled ascent, descent and cap height).

## Left out, on purpose

- Pinned tabs, tab groups, split tabs, the tab search button and tab
  scrolling: a terminal has none of them.
- `hover.opacity_by_width` and the GM2 contrast opacities: only used for
  image themes in Chrome.
- The hover card's fade and slide animations, its thumbnail crossfade,
  its two-layer shadow (drawn as rings of the same reach), its footer.
- `text.tab_title_elide_behavior` FADE_TAIL: the title is cut where the
  room ends, not faded.
- The close button's ripple (`close_button.ripple_timings`), the focus
  rings, keyboard focus.
- Wayland tiled edges (`frame.tiled_effects` is X11 only here).

## Known differences

- The toolbar's top line's rounded ends at the window's corners
  (`kToolbarCornerRadius` 8): the terminal's content is square.
- `gtk_frame.frame_thickness_dip_measurement` is done on a 2x render and
  ceiled; Chrome measures the 1x asset. A shadow ending on a half DIP can
  come out 1 DIP thicker.
- The favicon is the icon theme's terminal icon; Chrome's is the page's.
- Chrome's own frame is drawn from a 64-DIP-slice picture cut in nine,
  so a shadow's blur is exact only up to 64 DIP from a corner; the
  shadows reach 32 at most, so nothing is lost. The solid frame's
  interior line is blended over the strip's colour on its top row (the
  strip is opaque there), where Chrome draws it over the frame's own
  colour: the same pixel.
