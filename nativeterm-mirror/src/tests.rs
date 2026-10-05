use super::*;
use mux::renderable::{
    terminal_get_cursor_position, terminal_get_dimensions, terminal_get_dirty_lines,
    terminal_get_lines,
};
use native_term_remote::screen::{Copy, Since};
use std::sync::Arc;
use wezterm_term::{Terminal, TerminalConfiguration, TerminalSize};

#[derive(Debug)]
struct Config;

impl TerminalConfiguration for Config {
    fn scrollback_size(&self) -> usize {
        100
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

/// A terminal model fed bytes, read as a pane is.
struct Term(Terminal);

impl Term {
    fn new(cols: usize, rows: usize) -> Term {
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: cols * 8,
            pixel_height: rows * 16,
            dpi: 0,
        };
        Term(Terminal::new(
            size,
            Arc::new(Config),
            "WezTerm",
            "test",
            Box::new(Vec::new()),
        ))
    }

    fn print(&mut self, bytes: &str) {
        self.0.advance_bytes(bytes);
    }
}

impl Source for Term {
    fn dimensions(&mut self) -> RenderableDimensions {
        terminal_get_dimensions(&mut self.0)
    }

    fn cursor(&mut self) -> StableCursorPosition {
        terminal_get_cursor_position(&mut self.0)
    }

    fn title(&mut self) -> String {
        self.0.get_title().to_string()
    }

    fn alternate(&mut self) -> bool {
        self.0.is_alt_screen_active()
    }

    fn seqno(&mut self) -> SequenceNo {
        self.0.current_seqno()
    }

    fn changed_since(
        &mut self,
        rows: Range<StableRowIndex>,
        seqno: SequenceNo,
    ) -> RangeSet<StableRowIndex> {
        terminal_get_dirty_lines(&mut self.0, rows, seqno)
    }

    fn lines(&mut self, rows: Range<StableRowIndex>) -> (StableRowIndex, Vec<Line>) {
        terminal_get_lines(&mut self.0, rows)
    }

    fn palette(&mut self) -> ColorPalette {
        ColorPalette::default()
    }
}

/// The device's copy brought up to date from the mirror.
fn follow(mirror: &PaneMirror, copy: &mut Copy) {
    match mirror.mirror().since("tab", copy.seq) {
        Since::Nothing => {}
        Since::Rows(rows) => copy.rows(rows).unwrap(),
        Since::Snapshot(chunks) => chunks.into_iter().for_each(|c| copy.snapshot(c)),
    }
}

#[test]
fn text_colours_and_attributes_reach_the_device() {
    let mut term = Term::new(20, 5);
    // red, bold, curly underline, a link, then plain; a line too long for
    // 20 columns wraps
    term.print("\x1b[31;1mred\x1b[0m \x1b[4:3mwavy\x1b[0m ");
    term.print("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\r\n");
    term.print("0123456789abcdefghijKLM\x1b]0;my title\x07");
    let mut mirror = PaneMirror::default();
    assert!(mirror.refresh(&mut term));
    let mut copy = Copy::default();
    follow(&mirror, &mut copy);
    assert_eq!(copy.title, "my title");
    let runs = copy.runs(0).unwrap();
    let red = &runs[0];
    assert_eq!(red.0, "red");
    let (r, g, b, _) = ColorPalette::default()
        .resolve_fg(wezterm_term::color::ColorAttribute::PaletteIndex(1))
        .as_rgba_u8();
    assert_eq!(
        red.1.fg,
        0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
    );
    assert_ne!(red.1.attrs & Attr::Bold as u32, 0);
    let wavy = runs.iter().find(|(t, _)| t == "wavy").unwrap();
    assert_ne!(wavy.1.attrs & Attr::CurlyUnderline as u32, 0);
    let link = runs.iter().find(|(t, _)| t == "link").unwrap();
    assert_eq!(link.1.link, "https://example.com");
    assert_eq!(copy.text(1).unwrap().trim_end(), "0123456789abcdefghij");
    assert_eq!(copy.wrapped(1), Some(true), "the long line goes on");
    assert_eq!(copy.text(2).unwrap().trim_end(), "KLM");
    assert_eq!((copy.cursor.row, copy.cursor.col), (2, 3));
}

#[test]
fn only_changed_rows_are_read_and_sent() {
    let mut term = Term::new(30, 6);
    for i in 0..6 {
        term.print(&format!("line {i}"));
        if i < 5 {
            term.print("\r\n");
        }
    }
    let mut mirror = PaneMirror::default();
    mirror.refresh(&mut term);
    let mut copy = Copy::default();
    follow(&mirror, &mut copy);
    let before = copy.seq;
    // change row 2 only
    term.print("\x1b[3;1Hchanged");
    assert!(mirror.refresh(&mut term));
    match mirror.mirror().since("tab", before) {
        Since::Rows(rows) => {
            let indices: Vec<u64> = rows.rows.iter().map(|r| r.index).collect();
            assert_eq!(indices, [2], "the one row changed");
            copy.rows(rows).unwrap();
        }
        other => panic!("{other:?}"),
    }
    assert!(copy.text(2).unwrap().starts_with("changed"));
    // nothing new: nothing to send
    assert!(!mirror.refresh(&mut term));
}

#[test]
fn the_alternate_screen_and_a_new_size_start_over() {
    let mut term = Term::new(30, 6);
    term.print("shell prompt $ ");
    let mut mirror = PaneMirror::default();
    mirror.refresh(&mut term);
    let mut copy = Copy::default();
    follow(&mirror, &mut copy);
    // a full-screen program
    term.print("\x1b[?1049h\x1b[2J\x1b[Hvim");
    mirror.refresh(&mut term);
    assert!(matches!(
        mirror.mirror().since("tab", copy.seq),
        Since::Snapshot(_)
    ));
    follow(&mirror, &mut copy);
    assert_eq!(copy.screen, ScreenKind::ScreenAlternate as i32);
    assert!(copy.text(0).unwrap().starts_with("vim"));
    // and back, then a resize
    term.print("\x1b[?1049l");
    mirror.refresh(&mut term);
    follow(&mirror, &mut copy);
    assert_eq!(copy.screen, ScreenKind::ScreenNormal as i32);
    term.0.resize(TerminalSize {
        rows: 6,
        cols: 50,
        pixel_width: 400,
        pixel_height: 96,
        dpi: 0,
    });
    mirror.refresh(&mut term);
    assert!(matches!(
        mirror.mirror().since("tab", copy.seq),
        Since::Snapshot(_)
    ));
    follow(&mirror, &mut copy);
    assert_eq!(copy.cols, 50);
}
