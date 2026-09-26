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
margin); `ntb.position` (6 past the last tab, 6 down the strip) and
`ntb.button_size` 28 with its 16 icon and 14 radius; `region.grab_handle`
(42 DIP kept free after the new-tab button, before the caption buttons,
however full the strip: room to drag the window by);
`frame.corners_per_platform` (one rule of Chrome's, checked 2026-09-26:
the window's corners and shadow are the platform's where the platform
draws the frame — Windows 11's DWM rounds every ordinary top-level
window, Chrome sets no corner preference for browser windows and
neither does the fork, both measured round on the same desktop; macOS's
AppKit rounds its windows — and Chrome's own on Linux, where the top
corners are 8 DIP round when the window is composited (an ARGB visual
on X11; always on Wayland), not tiled, and Chrome draws the frame, the
bottom corners square, and the shadow (MD elevation 16 active, 2
inactive) drawn when the platform takes decoration insets
(`_GTK_FRAME_EXTENTS` on X11, `set_window_geometry` on Wayland) and is
composited, none of it when maximized, minimized, full screen or tiled;
the GTK frame's radius is the theme's. The fork's X11 and Wayland edges
follow the same conditions. EndeavourOS, KDE on Wayland, both windows
native Wayland: Chrome's and the fork's top-left corners round with the
same radius, bottom-left square; the fork's active shadow reached 17 px
left and 10 px up, Chrome's inactive one was invisible on the sky, as
elevation 2 is);
`region.caption_clicks` (the strip's empty part is the window's caption:
a press records the drag, which begins on motion past 8 DIP, Chrome's
Linux threshold, so a double-click's second press reaches the window and
toggles maximized on X11 and Wayland; the drag itself is then the
platform's: X11's _NET_WM_MOVERESIZE, Wayland's xdg move, and on macOS
`performWindowDragWithEvent:` with the press, as Chrome's
`cr_mouseDownOnFrameView` does for its caption; Windows has the strip as
HTCAPTION and does both itself; macOS's default double-click action, "Maximize",
is the zoom `maximize` performs — the Fill/Minimize/None preferences are
not read, see the known differences);
`tab_search.leading_button` (28 round, 6 into the region, the tabs' strip
28 past the region's start; the 16 chevron of `kExpandMoreOldIcon`); `px.scale_and_align_bounds` for the
body's pixel edges; the hit test's 3-DIP reach into the separators and
its rise to the top when maximized; `layout.overflow` (the tabs never
narrower than their minimums: a tab past the strip's trailing edge is
hidden whole, as is one before the active tab that would be past it were
it the active one — `TabContainerImpl::ShouldTabBeVisible`; Chrome has no
tab strip scrolling in this tree; the new-tab button then sits 6 past the
strip's own edge). Checked against Chrome itself on 2026-09-26
(EndeavourOS, a throwaway profile, 100 blank tabs, then three more opened
at the end, each active): the active tab past the edge is hidden, and the
new-tab button stays at the strip's edge with a gap before it of up to a
tab's width. Here the new-tab button follows the last tab shown instead
(see the known differences).

Contents: `favicon.size` 16 at the contents' corner (rows 12..28);
`tab.pre_title_padding` 8; `title.bounds` (8 before the close icon);
`title.cap_height_centring` (`RenderText::DetermineBaselineCenteringText`
over the 41-DIP tab; the font's whole height when it has no cap height);
`close_button.position` (icon 16 from the contents' right, button 28
around it, top 6); `close_button.show_rules` (the active tab always, an
inactive one with 68 of contents room, nothing under 32 wide);
`title.contrast` (the theme's text colour moved towards black or white,
whichever contrasts most with the tab, until it reaches Chrome's ratio
for the state — 10.46 active, 7.98 inactive, 5.0 and 4.5 in an unfocused
window, the unfocused colours first blended 75 % towards the tab:
`tab_strip_color_mixer.cc` `kTabFgToContrastMap`, `BlendForMinContrast`);
`favicon.centred_when_alone` (an inactive tab with room for nothing else
still shows its favicon, centred in the tab — `Tab::UpdateIconVisibility`
`center_icon_`; the active tab at 56 shows its close button alone);
`close_button.ink_drop_colours_opacities` (a circle of radius 8 around
the icon, 0.16 of the colour contrasting most with the tab);
`new_tab_button.hover_highlight` (`kColorTabStripControlButtonInkDrop`:
the colour contrasting most with the inactive tab's background — the
strip's, with a system theme — at 0.16; black on a light strip, white
on a dark one; the same on the tab search button; the Material
`kColorSysStateHeaderHover` override applies only without a custom
theme, which a system theme is).

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
`frame.tiled_effects` (one-way maximized, X11 and Wayland: the band alone, no shadow, no
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
resizing (`kResizeTopBorderThickness`); `_GTK_FRAME_EXTENTS` set to the
4-DIP borders whatever the window manager knows, as
`X11Window::UpdateDecorationInsets` sets it on mapping (seen on Chrome's
window on deepin: 4, 4, 0, 4). The content's top corners are cut square
(8 x 8) and show the picture there: the frame's colour, the line's top
run and the outline's arc, as Chrome's frame view paints under its tab
strip's clear corners. The
X11 window picks the variant from the same facts Chrome does
(`_GTK_FRAME_EXTENTS` in `_NET_SUPPORTED`, not Xfwm4; `_NET_WM_CM_S*`
owned); Wayland always has the shadow.

Hover card: `hover_card.card.client_width_dip` 256,
`hover_card.corner_radius_dip` 8, `hover_card.text.margins` 12,
`hover_card.show_delay.algorithm` (300 ms up to a 64-wide tab, 800 at
256, 500 more there), `hover_card.placement.anchor_rect` (the tab's
own left edge, 2 DIP up from its bottom); its animations
(TabHoverCardController with views::WidgetFadeAnimator's and
BubbleSlideAnimator's defaults): the first card fades in over 200 ms, a
card on another tab moves there at once, sliding 200 ms
(`kHoverCardSlideDuration`) while its text crossfades, a card taken away
fades out over 150 ms and shows again at once within 300 ms
(`kShowWithoutDelayTimeBuffer`), all FAST_OUT_SLOW_IN
(cubic-bezier(0.4, 0, 0.2, 1)); no slide with the fades (the declutter
features are off by default). Shown over an inactive window's tabs too.

Tab overflow: with more tabs than fit, the tabs shrink to their minimum
widths (active 56, inactive 32, overlap 18) and those past the strip's
end are hidden whole (`TabContainerImpl::ShouldTabBeVisible`), the
active one included: the default strip does not scroll
(`kTabStripUnification`, which brings `TabStripView`'s scrolling and
keeps the active tab in view, is disabled by default; checked
2026-09-26), and nothing else keeps the active tab shown.

Title: `text.tab_title_elide_behavior` FADE_TAIL (tab_title.cc): the
text runs to the label's edge, cut there, its last stretch faded out
linearly (RenderText::ApplyFadeEffects, CreateFadeShader) over
GetExpectedTextWidth(3), a third of the width when very narrow, to
transparent, or when fewer than four characters fit to up to 51/255 at
no width; the expected character width as gfx has it per platform (the
font's OS/2 xAvgCharWidth on Linux, 'x' on Windows, the mean of a-z on
macOS). Titles are not capped at `tab_max_width` cells.

Caption clicks: a double-click on the strip's empty part does what the
desktop's title bar preference says: GTK's `gtk-titlebar-double-click`
on Linux (toggle-maximize, minimize, lower, menu, none), passed in as
`titlebar_double_click`; macOS's `AppleActionOnDoubleClick` (Fill,
Maximize, Minimize, None), read by the window; Windows' caption.

Resizing on macOS (a live resize, zoom's animation): Chrome holds the
transaction that changes the window's frame until a frame of the new size
is drawn, up to 500 ms (CATransactionCoordinator's pre-commit handler,
NativeWidgetNSWindowBridge::ShouldWaitInPreCommit, kUIPaintTimeout), not
in a full screen transition. The window paints in `windowDidResize:`,
unthrottled, and the Metal layer presents with the transaction while the
size changes (`presentsWithTransaction`, through a vendored wgpu-hal
whose Metal surface flag is atomic, `deps/wgpu-hal`), so the frame and
the content change together.

Text: `text.gtk_font_name_parse` (the desktop font at Chrome's
whole-pixel size, sent by NativeTerm), `text.font_metrics_linux`
(ceiled ascent, descent and cap height).

## Left out, on purpose

- The Tab Search bubble (a WebUI page: a search box, the open tabs, the
  recently closed ones): the strip's tab search button opens the tab
  switcher (the Ctrl+Tab grid) instead.

- Pinned tabs, tab groups, split tabs, the tab search button and tab
  scrolling: a terminal has none of them.
- `hover.opacity_by_width` and the GM2 contrast opacities: only used for
  image themes in Chrome.
- The hover card's thumbnail crossfade, its two-layer shadow (drawn as
  rings of the same reach), its footer.
- The close button's ripple (`close_button.ripple_timings`), the focus
  rings, keyboard focus.

## Known differences

- The toolbar's top line's rounded ends at the window's corners
  (`kToolbarCornerRadius` 8): the terminal's content is square.
- Chrome falls back to the system frame (square corners, the window
  manager's decorations) on X11 window managers it does not know or
  that tile, and draws no shadow under Xfwm4 (`CanSetDecorationInsets`
  false there); the fork draws its own edge on any X11 window manager
  and trusts `_GTK_FRAME_EXTENTS` wherever it is advertised.
- With more tabs than fit, the new-tab button follows the last tab shown,
  where Chrome's stays at the strip's edge after a gap of up to a tab's
  width (the hidden tabs' room; most visible after tabs opened maximized
  and the window restored, the active tab at the end then hidden). Asked
  for 2026-09-26; `Layout::compute`'s `tabs_right`, under test.
- The caption's "menu" action shows the window manager's window menu
  (Wayland's `show_window_menu`, X11's `_GTK_SHOW_WINDOW_MENU`); Chrome
  shows its own frame menu where it has one and uses the server's only
  on Wayland. "lower" is X11 only, as in Chrome.
- The hover card's animations run a frame at a time at `max_fps`, the
  card's opacity applied to its quads; a colour emoji does not fade (the
  shader takes its alpha from the picture): at full opacity in a fading
  card, cut but not faded at a title's end.
- The window's round top corners cut the content by the arc's coverage
  (a mask multiplied into the content's corner squares, drawn last),
  as Chrome clips its painting to the rounded window shape: a button
  by the corner keeps its round highlight. Until 2026-09-26 the whole
  r x r corner square was cleared to the frame's picture beneath, which
  took a square bite out of the tab search button's and the close
  button's highlights (seen on Lingmo, radius 14).
- `gtk_frame.frame_thickness_dip_measurement` is done on a 2x render and
  ceiled; Chrome measures the 1x asset. A shadow ending on a half DIP can
  come out 1 DIP thicker.
- The favicon is the icon theme's terminal icon; Chrome's is the page's.
- The tab search button's icon is two overlapping windows, not Chrome's
  chevron: it opens the tab switcher's grid, not a menu (chosen
  2026-09-25).
- Where the window's caption buttons stand at the strip's leading end
  (macOS's traffic lights; a Linux layout with buttons on the left,
  elementary's close), the tab search button stands at the trailing end
  instead, 6 DIP after the new-tab button (the new-tab button's own
  distance from the tabs), and the tabs' room is 34 DIP the less for it
  where the leading button took 28 of it; the 42-DIP grab handle stays
  after it. Chrome keeps the button leading everywhere, its traffic
  lights included. One rule for every platform,
  `Config::caption_buttons_lead` (chosen 2026-09-26;
  `Inputs::tab_search_trailing`, under test).
- Chrome's own frame is drawn from a 64-DIP-slice picture cut in nine,
  so a shadow's blur is exact only up to 64 DIP from a corner; the
  shadows reach 32 at most, so nothing is lost. The solid frame's
  interior line is blended over the strip's colour on its top row (the
  strip is opaque there), where Chrome draws it over the frame's own
  colour: the same pixel.
