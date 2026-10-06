//! A pane's screen as NativeTerm's remote control keeps it
//! (`native_term_remote::screen::Mirror`, see NativeTerm's docs/REMOTE.md).
//!
//! What a pane shows is read through [`Source`]: a mux pane (the GUI's
//! panes in R1-R2, the mux server's from R3) or, in tests, a terminal model
//! fed bytes. [`PaneMirror::refresh`] reads only the rows that changed since
//! the last time (the pane's sequence numbers), turns them into the
//! protocol's rows (runs of one style, colours resolved through the pane's
//! palette as the desktop draws them) and gives them to the mirror, which
//! numbers what changed for the devices.

mod direct;
pub mod input;
pub mod marks;
mod relay;
pub mod server;
mod tabs;

use std::ops::Range;

use mux::pane::Pane;
use mux::renderable::{RenderableDimensions, StableCursorPosition};
use native_term_remote::proto::{Attr, Cursor, ScreenKind};
use native_term_remote::screen::{Line as RemoteLine, Look, Mirror, View};
use rangeset::RangeSet;
use termwiz::surface::{CursorVisibility, SequenceNo};
use wezterm_term::color::ColorPalette;
use wezterm_term::{CellAttributes, Line, StableRowIndex};

/// The mux server's workspace for detached tabs (NativeTerm's R3): their
/// panes live on in a window there, which no GUI shows; NativeTerm lists
/// them, and resumes one by moving it back into a window of its own.
pub const DETACHED_WORKSPACE: &str = "nativeterm-detached";

/// Scrollback rows kept for devices, above the screen.
pub const SCROLLBACK: usize = 2000;

/// Where a pane's screen is read from.
pub trait Source {
    fn dimensions(&mut self) -> RenderableDimensions;
    fn cursor(&mut self) -> StableCursorPosition;
    fn title(&mut self) -> String;
    fn alternate(&mut self) -> bool;
    /// The program asked for the mouse.
    fn mouse(&mut self) -> bool {
        false
    }
    fn seqno(&mut self) -> SequenceNo;
    fn changed_since(
        &mut self,
        rows: Range<StableRowIndex>,
        seqno: SequenceNo,
    ) -> RangeSet<StableRowIndex>;
    fn lines(&mut self, rows: Range<StableRowIndex>) -> (StableRowIndex, Vec<Line>);
    fn palette(&mut self) -> ColorPalette;
}

/// A mux pane as a source.
pub struct PaneSource<'a>(pub &'a dyn Pane);

impl Source for PaneSource<'_> {
    fn dimensions(&mut self) -> RenderableDimensions {
        self.0.get_dimensions()
    }

    fn cursor(&mut self) -> StableCursorPosition {
        self.0.get_cursor_position()
    }

    fn title(&mut self) -> String {
        self.0.get_title()
    }

    fn alternate(&mut self) -> bool {
        self.0.is_alt_screen_active()
    }

    fn mouse(&mut self) -> bool {
        self.0.is_mouse_grabbed()
    }

    fn seqno(&mut self) -> SequenceNo {
        self.0.get_current_seqno()
    }

    fn changed_since(
        &mut self,
        rows: Range<StableRowIndex>,
        seqno: SequenceNo,
    ) -> RangeSet<StableRowIndex> {
        self.0.get_changed_since(rows, seqno)
    }

    fn lines(&mut self, rows: Range<StableRowIndex>) -> (StableRowIndex, Vec<Line>) {
        self.0.get_lines(rows)
    }

    fn palette(&mut self) -> ColorPalette {
        self.0.palette()
    }
}

/// One pane's mirror, and where it last read the pane.
#[derive(Default)]
pub struct PaneMirror {
    mirror: Mirror,
    /// The pane's sequence number when last read (`None`: never).
    seqno: Option<SequenceNo>,
    /// The size and screen last read: another one means reading all.
    shape: Option<(usize, usize, bool)>,
}

impl PaneMirror {
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// Read what changed in the pane; returns whether the mirror changed.
    pub fn refresh(&mut self, source: &mut impl Source) -> bool {
        let dims = source.dimensions();
        let alternate = source.alternate();
        let shape = (dims.cols, dims.viewport_rows, alternate);
        let top = dims.physical_top;
        let bottom = top + dims.viewport_rows as StableRowIndex;
        let first = dims.scrollback_top.max(top - SCROLLBACK as StableRowIndex);
        let range = first..bottom;
        let now = source.seqno();
        let wanted: Vec<Range<StableRowIndex>> = match (self.seqno, self.shape == Some(shape)) {
            (Some(seqno), true) => source
                .changed_since(range.clone(), seqno)
                .iter()
                .cloned()
                .collect(),
            _ => vec![range.clone()],
        };
        let palette = source.palette();
        let mut rows = Vec::new();
        for want in wanted {
            let (from, lines) = source.lines(want);
            rows.extend(
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| line(from + i as StableRowIndex, l, &palette)),
            );
        }
        let cursor = source.cursor();
        let view = View {
            cols: dims.cols as u32,
            lines: dims.viewport_rows as u32,
            top: index(top),
            cursor: Cursor {
                row: index(cursor.y),
                col: cursor.x as u32,
                visible: cursor.visibility == CursorVisibility::Visible,
            },
            title: source.title(),
            screen: if alternate {
                ScreenKind::ScreenAlternate
            } else {
                ScreenKind::ScreenNormal
            },
            rows,
            mouse: source.mouse(),
        };
        self.seqno = Some(now);
        self.shape = Some(shape);
        let changed = self.mirror.update(view);
        // (the rows scrolled out of what devices keep)
        self.mirror.trim(index(first));
        changed
    }
}

/// A stable row index as the protocol's (they start at 0 and grow).
fn index(row: StableRowIndex) -> u64 {
    row.max(0) as u64
}

/// A line as the protocol's row: runs of one style each.
pub fn line(index_: StableRowIndex, line: &Line, palette: &ColorPalette) -> RemoteLine {
    RemoteLine {
        index: index(index_),
        wrapped: line.last_cell_was_wrapped(),
        runs: line
            .cluster(None)
            .into_iter()
            .map(|c| (c.text, look(&c.attrs, palette)))
            .collect(),
    }
}

/// A cell's attributes as the protocol's style: colours resolved through the
/// palette, as the desktop draws them (alpha 255).
pub fn look(attrs: &CellAttributes, palette: &ColorPalette) -> Look {
    let rgba = |c: wezterm_term::color::SrgbaTuple| {
        let (r, g, b, _) = c.as_rgba_u8();
        0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
    };
    let mut bits = 0u32;
    match attrs.intensity() as u32 {
        1 => bits |= Attr::Bold as u32,
        2 => bits |= Attr::Dim as u32,
        _ => {}
    }
    bits |= match attrs.underline() as u32 {
        1 => Attr::Underline as u32,
        2 => Attr::DoubleUnderline as u32,
        3 => Attr::CurlyUnderline as u32,
        4 => Attr::DottedUnderline as u32,
        5 => Attr::DashedUnderline as u32,
        _ => 0,
    };
    for (on, bit) in [
        (attrs.italic(), Attr::Italic),
        (attrs.reverse(), Attr::Inverse),
        (attrs.strikethrough(), Attr::Strikethrough),
        (attrs.invisible(), Attr::Invisible),
        (attrs.blink() as u32 != 0, Attr::Blink),
    ] {
        if on {
            bits |= bit as u32;
        }
    }
    let underline = attrs.underline_color();
    Look {
        fg: rgba(palette.resolve_fg(attrs.foreground())),
        bg: rgba(palette.resolve_bg(attrs.background())),
        attrs: bits,
        underline_color: if underline == wezterm_term::color::ColorAttribute::Default {
            0
        } else {
            rgba(palette.resolve_fg(underline))
        },
        link: attrs
            .hyperlink()
            .map(|h| h.uri().to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests;
